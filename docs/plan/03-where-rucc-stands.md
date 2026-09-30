# Where rucc stands

This is the inventory of what rucc has and lacks for the kernel, checked against the code at commit `df975c11` (0.17.0) on 30 September 2026. Most entries marked "probe" were confirmed by compiling a small file with a release build of rucc and reading the object back. The rest come from the code, with a path. Issue numbers are in `tamnd/rucc`.

Each gap has a size:

| Size | Meaning |
|---|---|
| S | a day or two |
| M | a week |
| L | several weeks |
| XL | a month or more |

Each gap also names the milestone from document 15 that needs it closed.

## 3.1 What is already good enough

These have evidence behind them, and we expect to spend time on them only when the kernel happens to find a bug.

- **The GNU C front end.**
  - Statement expressions, `typeof`, `__auto_type`, `__typeof_unqual__`, case ranges, range designators.
  - Computed goto with `&&a - &&b`, `__label__`, zero-length and flexible arrays, empty structs of size zero.
  - `?:` with the middle operand omitted, casts to union, anonymous structs and unions, and `-fms-extensions` tagged anonymous members.
  - `__int128`, bitfields of enum and `long` type, gnu89 `extern inline`, K&R definitions, attributes on labels and statements.
  - `_Pragma`, `__COUNTER__`, `__VA_OPT__`, `__has_include`, `__has_attribute`.
- **Attributes that work.** `weak`, `alias`, `visibility`, `used`, `noinline`, `always_inline`, `naked`, `aligned`, `packed`, `cleanup`, `mode`, `target`, `noreturn`, `gnu_inline`, `constructor` (`crates/rucc-sema/src/check/attr.rs`).
  - #542 and #1149 are still open, but inlining under `always_inline` works at every level.
- **Builtins that work.**
  - `choose_expr`, `types_compatible_p`, `offsetof`, `classify_type`, `object_size`, `dynamic_object_size`.
  - `expect`, `unreachable`, `trap`, `prefetch`, `assume_aligned`, `return_address(n)`, `frame_address(n)`.
  - bswap, ffs, clz, ctz, popcount, parity, the overflow builtins including the `_p` forms.
  - `__atomic_*` and `__sync_*` inline on x86-64 and AArch64.
- **Flags already accepted and honored.**
  - Frame and aliasing: `-fno-omit-frame-pointer`, `-fno-optimize-sibling-calls`, `-fno-strict-aliasing`, `-fno-strict-overflow`, `-fwrapv`.
  - Linkage and runtime: `-fno-common`, `-fno-builtin`, `-ffreestanding`, `-nostdinc`, `-nostdlib`, `-fno-asynchronous-unwind-tables`, `-funsigned-char`.
  - Code shape: `-falign-functions`, `-mno-red-zone`, `-fcf-protection` (which writes `endbr64` and the GNU property note), `-mfentry` and `-pg` on x86-64, `-fpatchable-function-entry=N,M` on both back ends with the `SHF_LINK_ORDER` section, `-fno-stack-clash-protection`, `-fno-ipa-sra`.
  - Diagnostics and paths: `-fstack-usage`, `-fmacro-prefix-map`, `-ffile-prefix-map`, `-include`, `-isystem`.
- **kbuild's driver surface.** `-Wp,-MMD,<file>` and `$(CC) -E -xc -` work. busybox 1.38.0, a kbuild project, builds and passes every cell in `rucc-real-corpus`. #863 is open but fixed in the code at `crates/rucc-driver/src/lib.rs:491`.
- **Local register variables.** `register long x asm("rsp")` inside a function works (`crates/rucc/tests/register_variables.rs`). #1673 looks stale.
- **The persona mechanism.** `-fgnuc-version=4.9.4` sets `__GNUC__`, `__GNUC_MINOR__` and `-dumpversion`. It already exists (`crates/rucc-session/src/lib.rs:1516`). What it does not do yet is in 3.2.
- **Custom sections from assembly.** `.pushsection __ex_table,"a"`, `.altinstructions`, `.data..percpu` and mergeable string sections come out right in objects written from `.S` input.
- **Speed and memory.**
  - The latest real corpus cost report (rucc 0.15.1 against GCC 16.2.0) shows a geomean compile time of 0.50x GCC at `-O2` and 0.77x at `-O0`.
  - #1233, the bison file that needed 22 GB, is closed.
  - The jtckdint outlier (#1957, one function of 190,000 instructions, 71 s against GCC's 2 s at `-O2`) is the known pathological shape. The kernel has a few generated functions of that kind, which document 12 watches.
- **Determinism.** `rucc-real-corpus` compiles twice and compares, and so does `rucc-postgres` with `RPG_TWICE=1`.

## 3.2 Identity and driver

| Gap | Where the kernel hits it | Issue | Size | Needed by |
|---|---|---|---|---|
| the first line of `--version` is `rucc 0.17.0`, which contains no lowercase `gcc` | 4.18 to 5.11 decide `CC_IS_GCC` with `grep -q gcc` on that line, so rucc fails with "unknown compiler" | none, open one | S | K1 |
| `-fgnuc-version` does not change the banner | `CONFIG_CC_VERSION_TEXT` and `/proc/version` | none | S | K1 |
| `-fgnuc-version` does not change the default dialect: with `-fgnuc-version=4.9.4`, `__STDC_VERSION__` is still `202311L` | every kernel to 3.17 relies on the gnu89 default, and old subtrees without `-std` meet C23 keywords | none | S | K1 |
| `-Wa,<anything>` is refused, since the assembler is inside the compiler | `as-version.sh` runs `$(CC) -Wa,--version`. The x86 and arm64 Makefiles pass `-Wa,--noexecstack`, `-Wa,-mx86-used-note=no`, `-Wa,--fatal-warnings`, `-Wa,-march=...` and `-Wa,-gdwarf-*` | none | S for an allow-list, M for honoring each | K1 |
| no `GNU assembler` banner under `-Wa,--version` | the kernel then says "unknown assembler invoked" and stops, from 5.12 | none | S | K1 |
| `-fno-PIE` (capitals) is an unknown option | passed on every x86 kernel from 4.9 | none | S, after `-fno-pie` | K2 |
| `-fshort-wchar`, `-fms-anonymous-structs`, `-mskip-rax-setup`, `-fmin-function-alignment`, `-fconserve-stack`, `-fno-partial-inlining`, `-fno-var-tracking-assignments`, `-fno-allow-store-data-races`, `-fmerge-constants`, `-fno-stack-check`, `-fzero-init-padding-bits=all`, `-fno-builtin-wcslen`, `-fno-lto`, `-mrecord-mcount`, `-mnop-mcount` and a dozen more are unknown | top level and arch Makefiles pass some of these unconditionally and probe others | none | S each, most are no-ops or one-line meanings | K1 and K2 |
| the target is not implied by the program name for the compiler, only for `dlltool` | `CROSS_COMPILE=aarch64-linux-gnu-` makes kbuild call `aarch64-linux-gnu-gcc` | none | S | K1 |
| `-print-file-name=plugin` must not return a usable directory | the GCC plugins must be detected as absent | check | S | K1 |
| `-Werror=<unknown>` refused, and no `-W` warning is ever emitted (#485) | `CONFIG_WERROR` is harmless when nothing warns. `-Wframe-larger-than` never firing means kernels with 2 KB frames build silently, and document 08.8 measures instead | #485 | S | K1 |

## 3.3 Front end, attributes and builtins

| Gap | Where the kernel hits it | Issue | Size | Needed by |
|---|---|---|---|---|
| `__attribute__((section("...")))` silently ignored | `__init`, `__initdata`, `__read_mostly`, `__section(".data..percpu")`, initcalls, `__param`, `__ksymtab`, and nearly every file | #909 | S to M | K2 |
| an integer constant expression cast to a null pointer is not a null pointer constant unless it is a literal `0` (`crates/rucc-sema/src/convert.rs:250`) | `__is_constexpr`, and through it `min()`, `max()`, `__must_be_array`, `BUILD_BUG_ON_ZERO` in older kernels | none, open one | S | K1 |
| a `const` variable is not an integer constant expression in `_Static_assert` (probe: E0614) | 7.2's minimum toolchain bump names this GCC behaviour as a dependency | none | S | K1 |
| `__builtin_constant_p` answered after the pass loop, so `if (!__builtin_constant_p(n) \|\| n < 8) ok(); else bad();` can leave a live call to `bad` after inlining | `BUILD_BUG_ON` inside inline helpers, the FORTIFY checks, `__bad_size_call_parameter` in `percpu.h`, `__xchg_wrong_size` | #604 closed, the late answer is #1768 and #1849 | M | K2 |
| `__attribute__((error("...")))` and `warning` ignored | `compiletime_assert` degrades to a link error naming `__compiletime_assert_123`, which is correct but useless for triage | none | S | K2 |
| `no_stack_protector` and `__attribute__((optimize("no-stack-protector")))` ignored | early boot, `start_secondary`, the x86 `cpu_startup_entry` path, all of which run before the canary exists | #811 | S | K5 |
| `__seg_gs` and `__seg_fs` named address spaces are a parse error | x86 percpu from 6.9 when `CC_HAS_NAMED_AS`. It is a probe, so a no is legal but must match GCC's yes at K5 | none | M | K5 |
| `counted_by`, `nonstring`, `designated_init`, `access`, `alloc_size`, `malloc`, `nonnull`, `no_sanitize*`, `nocf_check`, `zero_call_used_regs`, `function_return`, `indirect_branch`, `retain`, `externally_visible`, `noipa`, `flatten`, `copy`, `model`, `patchable_function_entry` accepted and ignored. Unknown attributes are accepted silently too (`attr.rs:37`) | most are hints, but `function_return("keep")`, `indirect_branch("keep")`, `nocf_check`, `zero_call_used_regs`, `no_sanitize` and `patchable_function_entry(0)` change code and are used on entry and mitigation paths | none | S each, M for the code-changing ones | K5 |
| `__has_attribute` answers 0 for attributes that work (`always_inline`, `noreturn`), and `crates/rucc-gnu/features.toml` is wrong in both directions | `compiler_attributes.h` (4.20 on) defines macros empty when `__has_attribute` says no, which silently drops `__always_inline` | none | S | K1 |
| `__builtin_has_attribute`, `__builtin_counted_by_ref` missing | newer `overflow.h` and `stddef.h` | none | S | K5 |
| `__atomic_signal_fence` refused | not used in the core tree as far as we know, check | none | S | check |
| cast as lvalue, `?:` and comma as lvalue, multi-line string literals, `-traditional` preprocessing | 2.4 and earlier, and some 2.6.early drivers | none | M | K11 |
| `__attribute__((regparm(3)))` warned and ignored | i386 kernels everywhere, `asmlinkage` flips it off | with i686 | M | K4 |

## 3.4 The assembler and inline assembly

This is the largest area, and document 07 treats it in full. rucc never runs an external assembler: a template is decoded into rucc's instruction tables or re-read by rucc's own `.s` reader (`crates/rucc-codegen/src/lower.rs:4403`). Every missing mnemonic, register or directive is therefore a hard error.

| Gap | Where the kernel hits it | Issue | Size | Needed by |
|---|---|---|---|---|
| `.macro`, `.endm`, `.irp`, `.irpc`, `.rept`, `.if*`, `.else`, `.altmacro`, `.purgem`, `\@` | `ALTERNATIVE`, `_ASM_EXTABLE_TYPE`, `SYM_FUNC_START`, `UNWIND_HINT`, the arm64 `alternative_cb` and `adr_l` macros, and every `.S` file through `linkage.h` | none | L | K2 |
| comparison operators in expressions, label differences as `.skip`, `.fill` and `.org` counts, 8-byte cross section differences | the `ALTERNATIVE` padding `.skip -(((new) - (old)) > 0) * ...` | none | M | K2 |
| an `asm goto` template with instructions in it | `static_branch_likely` through `arch_static_branch`, `unsafe_get_user`, `WARN_ON` on newer arm64 | #221 | L | K2 |
| a template with local labels and instructions that take operands in one block, such as `1: movl (%1),%0` plus `_ASM_EXTABLE(1b, ...)` | every `get_user` and `put_user`, `rdmsr_safe`, every exception table user | none | L | K2 |
| global register variables other than AArch64 `x18` (`crates/rucc-sema/src/check/decl.rs:1416`) | `current_stack_pointer` on x86 and arm64, `register ... asm("tp")` on riscv | none | M | K2 |
| x86 system instructions: `lgdt`, `lidt`, `ltr`, `wrmsr`, `rdmsr`, `swapgs`, `sysretq`, `iretq`, `cli`, `sti`, `hlt`, `invlpg`, `invpcid`, `xsave*`, `xrstor*`, `clac`, `stac`, `vmcall`, `wbinvd`, `rdpmc`, `in`, `out` and more | entry code, `paravirt`, `special_insns.h`, KVM | none | L | K2 |
| `%crN`, `%drN`, `%ss`, `%ds`, `%es` operands | `native_read_cr3`, the debug register code, segment loads at boot | none | M | K2 |
| absolute relocations: `$sym`, `sym(,%rax,8)`, `%gs:sym` (`crates/rucc-asm/src/instruction.rs:26`) | percpu access, `-mcmodel=kernel` addressing, jump tables in `.S` | none | M to L | K2 |
| `.code16`, `.code16gcc`, `.code32` | the boot setup, real mode trampoline, `head_64.S` 32-bit entry | none | XL (encoding) | K4 |
| `.section ...,"axG",@progbits,name,comdat` drops the group, `.linkonce` refused on ELF | `__x86_indirect_thunk_*` in `retpoline.S` are COMDAT on newer kernels | none | M | K5 |
| `.subsection`, `.text N` refused | older `spinlock` slow paths, `LOCK_SECTION` in 2.6 | none | S to M | K10 |
| `.nops`, `.reloc`, `.incbin` in `.S`, `.inst`, `.arch_extension`, `.cfi_undefined`, `.cfi_startproc simple` | alternatives padding, `static_call`, firmware blobs, arm64 `SYS_`/`__emit_inst`, entry unwind annotation | none | S each | K2, K3 |
| `.S` preprocessing: `__ASSEMBLER__` not predefined, `$SYM` not macro expanded, a `# comment` line is E0332 | every `.S` file | none | S each | K1 |
| AArch64 system instructions and system register names: `msr daifset`, `wfi`, `eret`, `hvc`, `smc`, `tlbi`, `ic`, `dc`, `at`, `hint`, `bti`, `pac*`, LSE `cas`, `ldadd`, `swp`, names such as `sctlr_el1`. Only nine EL0 names and the `s3_0_c1_c0_0` form work | everywhere in `arch/arm64` | none | L | K6 |
| AArch64 constraints beyond `r w Q m o V g X i n p I-N`, and `:abs_g0:`, `:abs_g1_nc:` modifiers | `arch/arm64` inline asm and `head.S` | none | S to M | K6 |
| x86 `%a`, `%z`, `%V`, `%l` modifiers outside `asm goto`, the `e` constraint with large constants | `%V` in the retpoline `CALL_NOSPEC`, `%a` in `bitops.h` on some versions | none | S | K2 |

## 3.5 Code generation

| Gap | Where the kernel hits it | Issue | Size | Needed by |
|---|---|---|---|---|
| `-mcmodel=kernel` refused, "small code model and no other" (`crates/rucc-driver/src/lib.rs:2205`) | every x86-64 kernel object | #11 | L | K2 |
| `-fno-pie`, `-fno-pic` refused, "position dependent code is not supported" (`lib.rs:1204`) | every x86-64 and i386 kernel object | #11 | M | K2 |
| no `R_X86_64_32S` (`crates/rucc-object/src/elf.rs:39`) | the kernel code model addresses everything with sign-extended 32-bit absolutes | #11 | with the code model | K2 |
| extern and weak addresses go through `GOTPCREL` even without PIC, leaving `.got` entries | x86 `vmlinux.lds.S` asserts `.got` is empty (from about 5.10, check), and weak undefined symbols such as `__start_*` are common | none | M | K2 |
| `-mno-sse`, `-mno-mmx`, `-mno-sse2` refused as "part of the baseline"; `-mno-80387`, `-mno-fp-ret-in-387`, `-msoft-float`, `-mgeneral-regs-only` unknown | every x86 and arm64 object. The compiler must never touch a vector or floating point register outside `kernel_fpu_begin` regions, not even for a struct copy or a `memset` | #11 | M to L | K2 |
| `-fstack-protector*` refused on AArch64 (`crates/rucc-target/src/aarch64.rs:835`); the guard fixed at `%fs:40` on x86-64 (`x86_64.rs:1326`); `-mstack-protector-guard=`, `-reg=`, `-offset=`, `-symbol=` unknown | x86-64 uses `%gs:__stack_chk_guard` (via `-symbol=__ref_stack_chk_guard` from 6.x), arm64 uses `sp_el0` plus an offset | #11 | M | K5 |
| no retpoline, return thunk or SLS: `-mindirect-branch=thunk-extern`, `-mindirect-branch-register`, `-mindirect-branch-cs-prefix`, `-mfunction-return=thunk-extern`, `-mharden-sls=all` unknown | `MITIGATION_RETPOLINE` and `MITIGATION_RETHUNK` are on in `defconfig`, and objtool validates them | #11 | M to L | K5 |
| `-fno-jump-tables` unknown, and jump tables are emitted (`crates/rucc-codegen/src/lower.rs:3195`) | IBT and retpoline configurations pass it, and objtool must be able to read any jump table it sees | none | S | K2 |
| `-mrecord-mcount` unknown, `-pg` refused on AArch64 | x86 ftrace up to the objtool mcount era, arm64 ftrace before patchable entries | none | S to M | K5 |
| `-fzero-call-used-regs`, `-ftrivial-auto-var-init` unknown | both probed, both on in hardened and distro configurations, `stackinit_kunit` tests the second | none | M each | K5 |
| `-mpreferred-stack-boundary=3` (x86-64) and `=2` (i386) unknown | stack alignment of 8, which changes frame layout and every `alignas(16)` local | none | M | K2 |
| small constant `__builtin_memcpy` and `memset` (8, 16, 64 bytes) become calls; a 4 KB `memset` is a call even under `-fno-builtin` | `noinstr` code must not call `memcpy` (objtool: "call to memcpy() leaves .noinstr.text section"), and performance | none | M | K3 |
| `-mcmodel`, `-mabi=lp64`, `-mno-outline-atomics`, `-mbranch-protection=`, `-ffixed-x18` on AArch64 | arm64 Makefile | none | S to M | K6 |
| `-fsanitize=kernel-address`, `thread`, `undefined`, `kcfi`, `shadow-call-stack` refused | KASAN, KCSAN, UBSAN configurations | none | XL each | K12, or stretch |
| no i386 back end; `-m32`, `-m16`, `-mregparm=3` refused | the x86-64 boot setup and trampoline (`-m16`), the 32-bit vDSO under `IA32_EMULATION` (`-m32`), every i386 kernel | M10, #11 | XL | K4 |
| no RISC-V back end (`crates/rucc-codegen/src/pipeline.rs:226`) | the R64 row | M9, #10 | XL | K7 |

## 3.6 Objects, debug information and the rest of the pipeline

| Gap | Where the kernel hits it | Issue | Size | Needed by |
|---|---|---|---|---|
| `.section .note.Linux,"a",@note` comes out as `SHT_PROGBITS` | `ELFNOTE` in `elfnote.h`: the Xen and PVH entry notes, the build ID, the Linux version note | none | S | K2. PVH boot needs it |
| declared but empty sections are dropped | linker scripts that `KEEP` a section and take its address | none | S | K2 |
| `SHF_GNU_RETAIN` not written for `retain` | `__used __retain` on newer kernels with `--gc-sections` (`LD_DEAD_CODE_DATA_ELIMINATION`) | none | S | K5 |
| objects in named sections may be over-aligned | linker-built arrays, where the kernel walks from `__start_x` to `__stop_x` with stride `sizeof(T)`. A compiler that aligns a 24-byte struct in a named section to 32, as some GCC versions did for "large" objects, breaks every one of them | check | S to M | K2 |
| DWARF 5 only; `-gdwarf-4` refused; `-gsplit-dwarf` refused; `-gz` a no-op | `DEBUG_INFO_DWARF4`, and pahole's DWARF reader for BTF, `gendwarfksyms` from 6.14 | M8, #9 | M | K5 |
| no DWARF validation against gdb or pahole | BTF from rucc DWARF must describe the same types GCC's does, or BPF programs will not load | none | M | K5 |
| no linker of our own | not a gap. The kernel calls `$(LD)` directly | | | |
| objtool has never read a rucc object | the unknown. Jump tables, noreturn calls, duplicated stack protector failure blocks, frame pointer setup, `noinstr` calls, missing thunks | #12 | L, and it could be much more | K3 |

## 3.7 Targets

| Target | State | Kernel role |
|---|---|---|
| `x86_64-linux-gnu` | tier 2, the best tested | X64 |
| `aarch64-linux-gnu` | back end complete, tier 4 in `docs/TARGETS.md` because nothing grades it | A64 |
| `i686-linux-gnu` | `Arch::X86` exists in `rucc-tuple`, the driver refuses the triple | X32, and the x86-64 boot code |
| `riscv64-linux-gnu` | parses, "there is no back end for riscv64" | R64 |
| `x86_64-none`, `aarch64-none` and the other `*-none` rows | tier 3 in the table. For the three without a back end that is wrong | the kernel does not use them. It builds with the `-linux-gnu` triple plus `-ffreestanding -nostdinc` and its own flags |

`docs/TARGETS.md` calls `x86_64-none` "the kernel target". This folder does not need it. The kernel is built with the Linux triple, because kbuild calls `$(CROSS_COMPILE)gcc` and never passes a `-none` target. The `-none` rows are for firmware and should drop to tier 4 until a back end exists.

## 3.8 Stale metadata worth fixing at K0

- #542, #1149, #863 and #1673 are open and appear fixed in the code.
- `crates/rucc-gnu/features.toml` claims statement expressions, `typeof`, case ranges, computed goto, zero-length arrays and `mode` are unimplemented. They work. It also claims `section` is an error, when in fact it is silently ignored.
- The tier 3 rows for `riscv64-none`, `i686-none` and `armv7*-none` are not backed by a back end.

## 3.9 The honest summary

For x86-64 `defconfig` at 7.2, the critical path is:
1. The assembler's macro language and x86 system instructions.
2. `asm goto` with bodies.
3. The kernel code model and its relocations.
4. `-mgeneral-regs-only`.
5. The `section` attribute.
6. `objtool`.
7. i686 for `-m16` and `-m32`, which the bring-up flag can defer.

Items 1 to 6 are about three to four months for one engineer, with `objtool` the least predictable. Everything else is tables of S items that parallelize well.
