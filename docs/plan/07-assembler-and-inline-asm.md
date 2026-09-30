# The assembler and inline assembly: the critical path

rucc assembles everything itself. Inline assembly templates are decoded by rucc's instruction tables, and `.S` files go through rucc's preprocessor and then its assembler (`crates/rucc-asm`). There is no fallback to gas. This is a strength: one parser, no process spawn, no text round trip, and it is part of why rucc compiles fast. For the kernel, though, it means rucc must be a complete GNU assembler for each architecture's kernel dialect.

This is the largest single piece of work in the plan and the one that gates K2.

## 7.1 `.S` preprocessing

| Item | GCC behaviour | rucc today | Fix |
|---|---|---|---|
| `__ASSEMBLER__` | defined when the input is `.S` or `-x assembler-with-cpp` | not defined, so every header's `#ifndef __ASSEMBLER__` block of C declarations leaks into assembly | define it. **S** |
| `$` in identifiers | `-E` for assembly treats `$` as part of identifiers only if allowed. The kernel uses `$SYM` for immediates on x86 (`movq $__START_KERNEL_map, %rax`) | `$SYM` not macro-expanded, so `$__PAGE_OFFSET` stays literal | in assembler-with-cpp mode, `$` is punctuation, as GCC's `-lang-asm` treats it. **S** |
| `#` at the start of a line that is not a directive | passed through as an assembler comment (`# comment` in x86 `.S`) | E0332 invalid directive | in assembly mode, unknown `#` lines pass through unchanged. **S** |
| `'` in comments and in `.ascii` context | no character constant errors in assembly mode | check | the lexer must be lenient about unmatched `'` in assembly mode, as GCC's is. **S** |
| `-traditional` | 1.x and 2.0 Makefiles pass it for `.S` | refused | old-style preprocessing, K11. **M** |
| `-D__ASSEMBLY__` | kernel passes it explicitly | fine | none |

All of these are K1, because `rk syntax` (K1's front end check) preprocesses every `.S` unit.

## 7.2 Directives

The kernel's assembly dialect is the GNU assembler's, and it uses nearly all of it. Here is the inventory from 7.2 `arch/x86` plus `arch/arm64` plus `include/`, by frequency, against rucc:

| Directive group | Kernel use | rucc | Size |
|---|---|---|---|
| sections: `.section name,"flags",@type`, `.pushsection`, `.popsection`, `.previous`, `.text`, `.data`, `.bss` | everywhere | yes, except the `G` group flag with a group name and `comdat` (dropped), and `@note` (written as `PROGBITS`) | S each |
| `.subsection N`, `.text N` | 2.6-era lock sections | refused | S to M |
| symbols: `.globl`, `.weak`, `.hidden`, `.type`, `.size`, `.set`, `.equ`, `=`, `.local`, `.comm`, `.lcomm` | everywhere | yes | |
| data: `.byte`, `.short`, `.word`, `.long`, `.int`, `.quad`, `.octa`, `.ascii`, `.asciz`, `.string`, `.fill`, `.skip`, `.space`, `.zero`, `.balign`, `.p2align`, `.align`, `.org` | everywhere | yes for constants and single symbols. **Not** for differences like `.quad b - a` across sections, `.long sym - .` in some forms, or `.skip` and `.fill` with a symbolic count | M |
| macros: `.macro`, `.endm`, `.exitm`, `\arg`, `\arg:req`, `\arg=default`, `\@`, `.purgem`, `.altmacro`, `.noaltmacro`, `LOCAL` | every x86 entry and every arm64 `.S` through `asm/assembler.h`, `SYM_*` from `linkage.h`, `ALTERNATIVE` | none | **L** |
| repetition: `.rept`, `.irp`, `.irpc`, `.endr` | `entry_64.S` register save loops, arm64 `assembler.h`, crypto | none | with macros |
| conditionals: `.if`, `.ifdef`, `.ifndef`, `.ifb`, `.ifnb`, `.ifc`, `.ifnc`, `.ifeq`, `.ifne`, `.else`, `.elseif`, `.endif` | everywhere macros are | none | with macros |
| expressions: `==`, `!=`, `<`, `>`, `<=`, `>=`, `&&`, `\|\|`, `!` (GNU `as` yields -1 for true), `%`, `<<`, `>>`, `^`, `~` | alternatives padding: `.skip -(((6651f-6641f)-(662b-661b)) > 0) * ((6651f-6641f)-(662b-661b)),0x90` | arithmetic only, no comparisons, and label differences not usable as absolute values until layout | **M**, needs the relaxation loop (7.3) |
| `.error`, `.warning`, `.print`, `.abort` | macro argument checks | none | S |
| CFI: `.cfi_*` | x86-64 up to 4.x (`CONFIG_AS_CFI`), arm64, all asm with `SYM_FUNC_START` | most. Missing `.cfi_undefined`, `.cfi_signal_frame`, `.cfi_startproc simple`, `.cfi_escape` | S each |
| `.nops N[, M]` | alternatives on 5.x and later | none | S |
| `.reloc offset, type, sym` | `static_call` sites, `ANNOTATE_*` in some versions | none | S to M |
| `.incbin` | firmware, certificates (`certs/system_certificates.S`), `kernel/configs.c` via `.S`, vDSO image embedding | check. Must support files relative to `-I` | S |
| `.code16`, `.code16gcc`, `.code32`, `.code64` | x86 boot, realmode trampoline, `head_64.S`, `efi_thunk_64.S` | none | **XL** (i686 encoder, K4) |
| `.arch`, `.arch_extension`, `.cpu` (arm64); `.option` (riscv) | arm64 LSE, crypto, SVE; riscv `norvc`, `push`, `pop` | partial | S to M each |
| `.inst` (arm64), `.insn` (riscv), `.byte`-encoded instructions (x86) | unsupported opcodes, `SYS_*` register encodings | check | S |
| `.linkonce` | old ELF kernels | refused on ELF | S |
| `.ident`, `.file`, `.loc`, `.size` expressions | everywhere | yes | |
| `.symver` | vDSO | check | S |
| `.hidden` on vDSO symbols, `.protected` | vDSO | check | S |

### 7.2.1 The macro processor

GNU `as` macro expansion is textual and runs before instruction parsing. The implementation belongs in `rucc-asm` as a separate layer: a line source that expands macros, repetitions and conditionals, feeding the existing statement parser. Requirements:
- **Arguments:**
  - positional and named (`foo x=1`);
  - `:req` and `:vararg`;
  - defaults;
  - `\()` as a separator;
  - `\@` as a unique counter per expansion;
  - the `.altmacro` `%expr` evaluation and `LOCAL` names.
- **Quoting:** arguments may contain commas when quoted, or in `.altmacro` mode inside `<...>`.
- **Conditionals evaluate expressions immediately.** Symbols must already be defined, or the expression is an error, as in gas.
- **`.ifc` and `.ifnc`** compare strings after expansion.
- **Nesting:** macros define macros, `.rept` inside `.macro`, and recursion up to a limit (gas: 100000 levels, practical kernel depth under 20).
- **Diagnostics** name the expansion chain, since errors inside `ALTERNATIVE` are unreadable otherwise.
- **Inline asm templates** go through the same layer, after operand substitution. The kernel defines macros in top-level `asm()` blocks and uses them in later inline asm, notably `ALTERNATIVE` on some versions and arm64's `__emit_inst`. So the macro table must be per translation unit and survive from one template to the next, in source order. This is how gas sees it, because GCC emits all of a unit's asm into one `.s` file.

That last point is the architectural catch. rucc currently treats each inline asm template as an isolated unit. It must keep one assembler state per object:
- macro definitions;
- `.set` symbols;
- section stack;
- `.altmacro` mode;
- `.arch` and `.option` state.

That state must be threaded through templates in the order GCC would have emitted them, which is function order and then top-level `asm()` order. **L. This is the biggest single item in K2.**

### 7.2.2 Expression evaluation and relaxation

GNU `as` allows expressions over labels whose values are unknown until branch relaxation has settled. The alternatives mechanism needs this:

```
661:	<old instructions>
662:
	.skip -(((6651f-6641f)-(662b-661b)) > 0) * ((6651f-6641f)-(662b-661b)),0x90
663:
	.pushsection .altinstructions,"a"
	.long 661b - .
	.long 6641f - .
	.word <feature>
	.byte 663b-661b
	.byte 6651f-6641f
	.popsection
	.pushsection .altinstr_replacement, "ax"
6641:	<new instructions>
6651:
	.popsection
```

The `.skip` count depends on instruction sizes in two sections, and those depend on relaxation. The assembler must:
1. Represent fragments with variable size (relaxable branches, and `.skip` with an expression).
2. Iterate to a fixed point, as gas does, with `.skip` treated as a fragment whose size is an expression over labels.
3. Evaluate comparisons to -1 or 0 (GNU truth values).
4. Report "non-constant expression" only after the fixed point, not before.

rucc's assembler has a relaxation loop for branches (check how general it is). It must be generalized to expression-sized fragments. **M.**

## 7.3 Inline assembly

| Feature | Kernel use | rucc | Size |
|---|---|---|---|
| `asm goto` with instructions in the template | `arch_static_branch` (`jmp` or `nop` plus a `__jump_table` entry), `unsafe_*_user`, `rmwcc`, `WARN_ON` on some arches, `cpu_feature_enabled` via `static_cpu_has` | fails (`crates/rucc-codegen/src/lower.rs:6006`, #221) | **L** |
| `asm goto` with outputs | `CC_HAS_ASM_GOTO_OUTPUT` users: `get_user`, `__get_user_asm`, `unsafe_get_user` on 5.8+ | none | M after the above |
| local labels (`1:`, `1b`, `1f`) together with operands in one template | every `_ASM_EXTABLE`, every exception fixup, `rdmsr_safe`, `cmpxchg` loops on arm64 LL/SC | fails | **L**, same root cause |
| one memory operand per template only | `cmpxchg_double`, `xadd` with two memory operands, `__put_user_asm` with an input and a memory output | limited | M |
| global register variables | `current_stack_pointer`: `register unsigned long current_stack_pointer asm("rsp")` (x86), `asm("sp")` (arm64), `asm("tp")` (riscv) | refused except aarch64 `x18` | M |
| x86 modifiers `%a`, `%b`, `%h`, `%w`, `%k`, `%q`, `%c`, `%P`, `%p`, `%n`, `%V`, `%l`, `%z`, `%H` | `%V` in `CALL_NOSPEC`, `%c` and `%P` for `__ASM_FORM` constants, `%a` for addresses, `%l` for labels, `%z` for size suffixes | `%a`, `%z`, `%V`, `%l` missing | S |
| x86 constraints `a b c d S D r q Q R l m o V < > g X i n I J K L M N e Z` plus `A`, `t`, `u` (x87), `x`, `y` | `e` (sign-extended 32-bit immediate), `Z`, `N` (8-bit port), `A` (edx:eax on i386), `=@cc<cond>` flag outputs (4.12+) | check `e`, `Z`, `A`, `@cc` | S each |
| arm64 constraints and modifiers `%w`, `%x`, `%c`, `%a`, `Q`, `Ump`, `K`, `L`, `Ush`, `S` | `Q` for exclusives, `%w` for 32-bit views, `S` for symbol references | partial | S to M |
| `asm inline` | 5.4+ everywhere through `asm_inline` | check | S |
| `asm volatile` ordering and `"memory"` clobbers | everywhere | yes, but must be audited against the memory model (06.7) | |
| the template's size estimate for inlining | GCC counts instructions by lines and `;` | irrelevant to correctness, relevant to 12 | |

### 7.3.1 Why templates with labels and operands fail

The current design substitutes operands and then parses the template as a list of instructions to lower into rucc's machine IR (`lower.rs:4403`). Label definitions and references inside the template need the local-label numbering of the whole object. Numeric labels like `1:` may repeat across templates and functions, and `1b` means the nearest previous one in the output stream. So a template cannot be resolved in isolation, even though the instructions themselves could be.

The fix is the same as for macros: templates become text fragments in one assembler stream per object, placed in the function's instruction stream at the right point. The assembler resolves numeric labels over the stream. In machine IR, a template becomes an opaque block of known register effects (from the constraints) and unknown size, relaxed together with the function's branches.

`asm goto` labels are C labels, whose machine basic blocks the template's `%l[name]` refers to. The block becomes an IR terminator with those successors plus the fall-through. That is what #221 designs.

### 7.3.2 The exception table and friends

`_ASM_EXTABLE_TYPE(from, to, type)` expands to:

```
.pushsection "__ex_table","a"
.balign 4
.long (from) - .
.long (to) - .
.long type
.popsection
```

The requirements:
- PC-relative 32-bit relocations from a data section into text, `R_X86_64_PC32` against local labels, which rucc can write;
- `.pushsection` inside a template, which works;
- the local labels, which do not yet work.

`__jump_table`, `.altinstructions`, `.static_call_sites`, `__bug_table`, `.retpoline_sites`, `.return_sites`, `.ibt_endbr_seal`, `.discard.*` annotations and `__patchable_function_entries` all follow the same pattern. Once 7.3.1 is fixed, they all work, and 09.4's section differential verifies that they do.

## 7.4 Instructions and registers

### 7.4.1 x86-64 and i686

rucc's x86-64 instruction tables cover what its code generator emits plus a user-mode set for inline asm. The kernel needs the system set. From an inventory of 7.2 `arch/x86` plus the tree's inline asm:

| Group | Instructions |
|---|---|
| control registers and descriptor tables | `mov %crN`, `mov %drN`, `lgdt`, `lidt`, `sgdt`, `sidt`, `lldt`, `sldt`, `ltr`, `str`, `lmsw`, `smsw`, `clts` |
| MSRs and CPU state | `rdmsr`, `wrmsr`, `wrmsrns`, `rdtsc`, `rdtscp`, `rdpmc`, `rdpid`, `cpuid`, `xgetbv`, `xsetbv`, `rdfsbase`, `wrfsbase`, `rdgsbase`, `wrgsbase` |
| mode switches | `swapgs`, `sysret`, `sysretq`, `sysexit`, `iret`, `iretq`, `lret`, `ljmp`, `lcall`, `int`, `int3`, `into`, `ud0`, `ud1`, `ud2`, `hlt`, `cli`, `sti`, `pushf`, `popf`, `eretu`, `erets` (FRED, 6.9+) |
| paging and caches | `invlpg`, `invlpga`, `invpcid`, `invlpgb`, `tlbsync`, `wbinvd`, `wbnoinvd`, `invd`, `clflush`, `clflushopt`, `clwb`, `prefetchw` |
| I/O ports | `in`, `out`, `ins`, `outs` in all widths |
| segments | `mov %ds`/`%es`/`%ss`/`%fs`/`%gs`, `push`/`pop` of segment registers, `lsl`, `lar`, `verr`, `verw` |
| extended state | `xsave`, `xsaveopt`, `xsavec`, `xsaves`, `xrstor`, `xrstors`, `fxsave`, `fxrstor`, `fninit`, `fnstcw`, `fldcw`, `ldmxcsr`, `stmxcsr`, `vzeroupper` |
| SMAP, CET, SGX, TDX, SEV | `clac`, `stac`, `endbr64`, `endbr32`, `incsspq`, `rdsspq`, `saveprevssp`, `rstorssp`, `setssbsy`, `clrssbsy`, `wrss`, `wruss`, `encls`, `enclu`, `enclv`, `tdcall`, `seamcall`, `vmgexit`, `pvalidate`, `rmpadjust`, `rmpupdate`, `psmash` |
| VMX and SVM | `vmxon`, `vmxoff`, `vmlaunch`, `vmresume`, `vmread`, `vmwrite`, `vmptrld`, `vmptrst`, `vmclear`, `invept`, `invvpid`, `vmcall`, `vmmcall`, `vmrun`, `vmload`, `vmsave`, `stgi`, `clgi`, `skinit` |
| misc | `monitor`, `mwait`, `monitorx`, `mwaitx`, `umonitor`, `umwait`, `tpause`, `serialize`, `pconfig`, `enqcmd`, `enqcmds`, `movdir64b`, `movdiri`, `lfence`, `sfence`, `mfence`, `pause`, `rdrand`, `rdseed`, `ptwrite`, `xbegin`, `xend`, `xabort`, `xtest` |
| crypto `.S` (only with `CRYPTO_*` on in the config; `allmodconfig` has them all) | AES-NI, `pclmulqdq`, SHA-NI, AVX, AVX2, AVX-512 (EVEX, masks, broadcasts), GFNI, VAES, VPCLMULQDQ, ADX, BMI2, including the AVX10 forms in 6.x |

Every instruction must be encoded exactly as gas encodes it, including prefix order and the choice among equivalent encodings. Otherwise:
- `alternatives` replacement lengths change;
- `objtool`'s decoder, which is table-driven from the same opcode map, may reject the form;
- byte comparisons in 7.6 fail.

The inventory is generated, not hand written. `rk asm-inventory` runs gas over every `.S` unit and every inline-asm-bearing unit's `-S` output from the reference build, and collects the mnemonics and operand shapes. Each missing shape becomes a row in the corpus of 7.6. **L for the tables, and the AVX-512 encoder alone is M.**

Operands the tables must add:
- `%crN`, `%drN` and the segment registers;
- absolute memory (`sym`, `sym(,%reg,8)`, `sym+off(%reg)`), `%gs:sym` and `%gs:sym(%reg)`, which is how every percpu access looks;
- `$sym` immediates with `R_X86_64_32S`, `R_X86_64_32` and `R_X86_64_64`;
- `lcall`/`ljmp` `$seg,$off`.

### 7.4.2 AArch64

From the arm64 inventory:
- **System registers**, about 300 names used by name in 7.2, from `sctlr_el1`, `tcr_el1`, `ttbr0_el1`, `vbar_el1`, `mair_el1`, `esr_el1`, `far_el1`, `elr_el1`, `spsr_el1`, `sp_el0`, `daif` and `currentel` to the `id_aa64*` registers, the PMU, GIC and timer registers and the EL2 registers.
  - GNU `as` knows them by name. The kernel increasingly uses `sysreg.h`'s generated `SYS_*` encodings through `msr_s`/`mrs_s`, which are macros emitting `.inst`, and those need the macro processor, not a table.
  - The names must still be known for the rest.
  - The table is generated from binutils' `aarch64-sys-regs.def`, with its license checked.
- **System instructions:** `msr daifset`, `msr daifclr`, `msr pan`, `msr uao`, `msr ssbs`, `isb`, `dsb`, `dmb` with all options, `wfi`, `wfe`, `sev`, `sevl`, `yield`, `eret`, `eretaa`, `eretab`, `hvc`, `smc`, `svc`, `brk`, `hlt`, `hint #n`, `bti`, `paciasp`, `autiasp`, `pacia`, `autia`, `xpaclri`, `retaa`, `tlbi` (all forms), `ic`, `dc`, `at`, `sys`, `sysl`, `clrex`, `esb`, `psb`, `tsb`, `csdb`, `sb`, `wfet`, `wfit`.
- **Extensions:**
  - LSE atomics (`cas`, `casp`, `ldadd`, `ldclr`, `ldeor`, `ldset`, `swp`, `stadd` and all ordering suffixes);
  - LSE2 / RCpc (`ldapr`, `stlur`);
  - MTE (`irg`, `stg`, `ldg`, `st2g`, `stzg`, `gmi`, `subp`);
  - SVE and SME instructions in the save/restore paths (`fpsimd.S`, `sve.S`, `sme` in 5.19+): `ldr z`, `str z`, `ldr p`, `str p`, `rdvl`, `smstart`, `smstop`, `ldr za`, `str za`, `zero {za}`;
  - GCS (6.13+), with FEAT_GCS instructions and registers;
  - crypto (AES, SHA1/2/3, SHA512, SM3, SM4, PMULL), NEON in full.

**L for arm64 in total, most of it table generation.**

## 7.5 Relocations and object details

| Item | Needed for | Size |
|---|---|---|
| `R_X86_64_32S`, `R_X86_64_32`, `R_X86_64_64` against symbols and sections | `-mcmodel=kernel`, `$sym`, `.quad sym` (existing) | M, with 08.2 |
| `R_X86_64_PC32` against weak and undefined symbols, without a GOT | 08.2 | with 08.2 |
| `R_X86_64_PLT32` for calls (binutils 2.31+ prefers it) | objtool expects `PLT32` for calls from 4.19ish on | check |
| `R_X86_64_GOTPCREL`/`REX_GOTPCRELX` never in kernel mode | the `.got` assertion | S |
| `R_386_*` | i686 | with K4 |
| `R_AARCH64_*` absolute, `ADR_PREL_PG_HI21`, `ADD_ABS_LO12_NC`, `LDST*_ABS_LO12_NC`, `CALL26`, `JUMP26`, `PREL32`, `PREL64`, `MOVW_UABS_G*`, `ABS64` | arm64 kernel, `adr_l`, `ldr_l`, `:abs_g*:` | check |
| COMDAT groups (`G`, `comdat`) and `SHT_GROUP` | retpoline thunks, some vDSO builds | M |
| `SHT_NOTE` for `@note` | `ELFNOTE` | S |
| empty sections kept | `KEEP` in linker scripts | S |
| `SHF_LINK_ORDER` via `"ao"` with a linked section | `__patchable_function_entries` (done), `.discard` annotations | check |
| `SHF_GNU_RETAIN` via `"R"` | `__retain` | S |
| `.note.GNU-stack` present on every object (kernel passes `-Wa,--noexecstack`) | `ld` warnings, which `CONFIG_WERROR` makes fatal from 6.x (`-z noexecstack` warnings) | check |
| section symbol vs local symbol relocations, as gas chooses them | objtool and the alternatives code read relocations and expect gas's choices in places (objtool: "can't find reloc entry symbol") | M, found by 7.6 |

## 7.6 The assembler corpus (`kernel-asm` in rucc-compat)

We cannot test the assembler by booting kernels. Too many things fail at once, and the failure is a hang. Instead:

1. For each graded version and architecture, take every `.S` unit in the `allmodconfig` build. Use its preprocessed form from the reference build, `make <obj>.s` for `.S` units.
2. Also take every C unit's `-S` output from the **reference GCC**. That is gas input exercising every inline asm template exactly as GCC expanded it, with all of GCC's directives.
3. Assemble each with gas (the reference binutils) and with rucc's assembler, driven as `rucc -c -x assembler`.
4. Compare the two objects section by section: the same section names, flags, types, sizes, bytes, the same symbols with the same binding and value, and the same relocations. There is an allowed normalization for section symbol versus local symbol choices, and for `.comment` and `.note.gnu.property` contents.

Step 2 is what makes this powerful. It tests rucc's assembler on the exact text the kernel's inline asm becomes under GCC, independently of rucc's compiler. Every macro, every alternative, every local label and every relocation form the kernel uses appears in it. On 7.2 x86-64 `allmodconfig` it is about 27,000 `.s` files. They cannot be committed as a corpus, for size and license reasons, so `rucc-compat` carries the generator and a manifest of hashes. The nightly regenerates them from the pinned tree.

The pass rate per version, per architecture, and per missing feature (bucketed by the assembler's error message) is the progress bar for K2's hardest part. The exit criterion: 100% byte-identical, or explained, for x86-64 `defconfig` on 7.2.x and 6.12.y, and for every `.S` in `allmodconfig`.

## 7.7 Order of work

1. `.S` preprocessing items (7.1). S each, K1.
2. Expression evaluation with comparisons and the relaxation generalization (7.2.2). M.
3. The macro processor (7.2.1). L.
4. One assembler stream per object, templates as fragments, numeric labels across templates (7.3.1). L.
5. `asm goto` as a terminator (#221). L.
6. x86 system instructions and operands, generated from the 7.6 inventory (7.4.1). L.
7. Relocations, notes, groups (7.5). M.
8. arm64 system set (7.4.2). L, K6.
9. `.code16gcc` and i386 encoding, K4.

Items 2 to 7 are about 14 weeks for one engineer, and they are most of K2's cost. Items 3, 4 and 5 touch the same code in `rucc-asm` and `rucc-codegen/src/lower.rs`, and should be one person's work.
