# objtool, linking and images

Between rucc's objects and a bootable kernel there are several consumers:
- `objtool`, which reads every object;
- `modpost`;
- `ld` with the kernel's linker script;
- `kallsyms`, `sorttable` and `resolve_btfids`;
- `pahole`;
- the boot image tools.

Each consumer makes assumptions about what a compiler emits. This document lists them.

## 9.1 objtool

Since 4.6, x86 builds run `tools/objtool/objtool check` on every object, and from 5.x on `vmlinux.o` as a whole (`--link`, `noinstr` validation). It decodes every function, builds a control flow graph, and checks:

| Check | Since | What it wants from the compiler |
|---|---|---|
| stack frame validation | 4.6 | with `FRAME_POINTER`, the frame is set up before any call. Without it (ORC), stack state is trackable: every `%rsp` change is by an immediate or a `lea` of known form, or `%rbp` restores it. No `and $-16, %rsp` without a frame pointer (the DRAP pattern is recognized in GCC's form only) |
| unreachable instructions | 4.6 | no code after a noreturn call. objtool has a list of noreturn functions (`noreturns.h` in 6.x) and must agree with the compiler's |
| sibling calls and returns | 4.6 | tail calls are `jmp sym`, not `jmp *%reg` (retpoline) |
| jump tables | 4.6 | objtool recognizes GCC's (and Clang's) switch table pattern: `jmp *table(,%reg,8)` with a `.rodata` table of `R_X86_64_64` entries adjacent to the text relocation, or `mov table(,%reg,8), %reg; jmp *%reg`. Any other pattern means "sibling call from callable instruction with modified stack frame" or "unreachable instruction" |
| ORC generation | 4.14 | objtool generates `.orc_unwind` from its stack tracking. What it cannot track, it cannot unwind |
| `noinstr` | 5.8 | no call from `.noinstr.text` to instrumentable text. Compiler-generated calls to `memcpy`, `memset`, `__stack_chk_fail`, `__sanitizer_*` or `__fentry__` in a `noinstr` function are violations |
| uaccess | 5.2 | between `stac` and `clac`, no call to a function not on the allowed list |
| retpoline, rethunk, SLS, IBT | 4.15 to 5.18 | 08.6 |
| `--mcount`, `--static-call`, `--orc`, `--uaccess`, `--prefix` | 5.x, 6.x | call site collection |
| `--link --noinstr` over `vmlinux.o` | 5.10 | the whole-image cross checks |

The build fails on objtool errors, and on warnings when `CONFIG_WERROR` or `OBJTOOL_WERROR` (6.x) is on, and our grade treats warnings as failures anyway. Since objtool has seen only GCC and Clang output, the risk is that rucc's shapes are valid but unrecognized:

1. **Jump table layout.** rucc lowers switches to tables (`lower.rs:3195`). objtool's `find_jump_table` wants a specific relocation shape. The alternatives are to emit GCC's shape exactly, or to use `-fno-jump-tables` everywhere (the IBT and retpoline builds pass it anyway). Emitting GCC's shape is the right answer for non-retpoline builds, and for old kernels without retpoline.
2. **Noreturn calls.** rucc must not emit code after a call to a function it knows is noreturn (`__attribute__((noreturn))`, `abort`, `__stack_chk_fail`, `__builtin_unreachable` blocks). The block must end, with at most an `int3` or `ud2`. objtool must also agree: a function rucc thinks may return, and objtool thinks noreturn, gives "unreachable instruction" if rucc emits a fall-through. Rule: rucc treats exactly the kernel's noreturn declarations as noreturn and never infers noreturn from a function body across units.
3. **Frame setup order.** With `FRAME_POINTER`, `push %rbp; mov %rsp,%rbp` must come before the first call. rucc's shrink-wrapping, if any (#1178's machine passes), must not move the frame setup below a call, and must not split it when frame pointers are required. objtool warns "call without frame pointer save/setup".
4. **Stack realignment.** DRAP-style realignment for over-aligned locals (08.4) has two GCC forms objtool recognizes. rucc must match one of them.
5. **Cold partitioning.** GCC emits `.text.unlikely` / `foo.cold` parts, which objtool handles by name (`.cold` suffix). If rucc splits functions, it must use the same naming. Otherwise, don't split.
6. **Duplicate stack protector failure paths**, and `ud2` versus `call __stack_chk_fail`: either is fine if noreturn is known.
7. **`__builtin_unreachable` at the end of a function** with no terminator. objtool: "falls through to next function". rucc must emit `ud2` or `int3` (GCC emits nothing, and objtool special-cases GCC via annotations; check what 7.2 wants), or better, make the preceding block's branch not fall through.

**How we find out:**
- At K2, run objtool over every rucc object of 7.2 `defconfig` as a standalone step, even before vmlinux links. Bucket the warnings by message and function shape.
- Each bucket gets a rucc issue with a reduced object pair, and the rucc-corpus facet `objtool-shapes` holds them.
- The size of the objtool gap is the largest single unknown in the plan (open question 2). If it is worse than expected, the fallback is to talk to the objtool maintainers about a documented annotation (`ANNOTATE_*`), not to patch objtool. We never patch it in the harness.

**L, K3.** The exit criterion: zero objtool warnings on x86-64 `defconfig` for 7.2.x, 6.12.y and 6.1.y.

## 9.2 Linker and linker scripts

The kernel links with `$(LD)` directly: GNU ld (`ld.bfd`) or `ld.lld`, never through the compiler driver. Graded builds use GNU ld from the era's binutils. On Current, `ld.lld` from LLVM 21+ is a second, ungraded column, reported because `LLVM=1` users link with it and a rucc object that only GNU ld accepts is still a finding. That is open question 3.

What the linker script and `ld` demand of objects:

| Demand | rucc |
|---|---|
| named sections land where the script puts them (`.init.text`, `.data..percpu`, `__ksymtab+sym`, `.rodata.str1.1`, `.text.hot`, `.text.unlikely`, `.noinstr.text`, `.entry.text`, `.head.text`) | needs `section` attribute (#909) |
| no orphan sections. `--orphan-handling=warn` (5.10+) with `CONFIG_WERROR` makes an orphan fatal | rucc must not invent sections GCC would not, such as `.text.<fn>` without `-ffunction-sections`, `.rodata.cst16` for general-register-only builds, or `.note.rucc`. `rk sections-diff` 9.4 lists every section name in rucc's objects missing from GCC's |
| `-ffunction-sections -fdata-sections` naming (`.text.<fn>`, `.data.<var>`, `.bss.<var>`, `.rodata.<var>`, `.text.unlikely.<fn>`) for `LD_DEAD_CODE_DATA_ELIMINATION` and FG-KASLR proposals | check naming identity with GCC |
| `.rodata.str1.1` mergeable strings with `SHF_MERGE\|SHF_STRINGS` | check |
| linker arrays: `__start_X`/`__stop_X`, `KEEP(*(X))`, walked by stride | no over-alignment of objects in named sections (03.6). rucc must align a variable in a named section to its type's alignment, or its `aligned` attribute, and no more. GCC aligns large objects to 32 at `-O2` for vectorization, except where the kernel's `__aligned` or `__section` fixes it. The kernel works around GCC's behaviour in places, which means rucc must match GCC, not the ideal |
| `.discard.*` and `/DISCARD/` sections consumed by objtool and dropped | fine |
| `ASSERT`s in `vmlinux.lds.S`: `.got` and `.plt` empty, `.rela.*` empty on x86-64, kernel image size, `__per_cpu` layout | 08.2 |
| `-z noexecstack` warnings (binutils 2.39+) | `.note.GNU-stack` on every object |
| relocatable links (`ld -r`) for `built-in.a` / `vmlinux.o` and modules | standard ELF, fine |

## 9.3 After the link

| Step | Tool | What it reads from rucc's output |
|---|---|---|
| `kallsyms` | `scripts/kallsyms` | `nm` output of vmlinux. Symbol names, types, order. rucc's local symbol naming for statics (`foo.0`, `foo.constprop.0`, `foo.isra.0`, `foo.cold`) appears in `/proc/kallsyms` and in stack traces. Differences are cosmetic, but `kallsyms` self tests and `livepatch` look up names. Report, don't grade |
| `sorttable` | `scripts/sorttable` | sorts `__ex_table`, `.orc_unwind_ip` and `__mcount_loc` in place. Needs them well formed |
| `resolve_btfids` | 5.7+ | `.BTF_ids` section from `BTF_ID()` macros (asm in a C file), and the BTF from pahole |
| `pahole -J` | 5.2+ with `DEBUG_INFO_BTF` | rucc's DWARF. Every type must be described as GCC would. Differences in BTF break BPF CO-RE programs against the kernel |
| `gendwarfksyms` | 6.14+ with `MODVERSIONS` and Rust, or opt-in | DWARF instead of `genksyms` for CRCs. The CRCs then depend on DWARF equality |
| `modpost` | all | reads `__ksymtab*`, `.modinfo`, `__versions`, section mismatch checks (`.init.text` referenced from `.text`). Section mismatch warnings must be identical, which means rucc must not inline an `__init` function into non-init code or keep a non-init copy. GCC's behaviour is that `__init` functions are only inlined into `__init` callers (`noinline` is not implied, but section attributes block inlining across sections) |
| `vdso2c`, `relocs` | x86 | `vdso2c` wants the vDSO image to be a clean shared object with a fixed set of sections. `arch/x86/tools/relocs` reads vmlinux relocations for KASLR and rejects unexpected types ("Unsupported relocation type") |
| `objcopy`, `strip`, `nm`, `readelf` from binutils | all | standard |

## 9.4 The section differential (`rk sections-diff`)

This is the check that would have caught CCC's x86 failure, its 40,000 undefined references to `__jump_table` and `__ksymtab` entries. For every object in both builds, and for `vmlinux`, `rk sections-diff` compares:

| Measure | Rule |
|---|---|
| set of section names, ignoring per-function names under `-ffunction-sections` | equal. Extra or missing names are failures |
| entry counts of the kernel's tables: `__ksymtab`, `__ksymtab_gpl`, `__kcrctab*`, `.init.setup`, `.initcall*.init`, `__param`, `.con_initcall.init`, `__tracepoints*`, `_ftrace_events`, `__syscalls_metadata`, `.BTF_ids` | equal per object |
| the call-site lists the compiler writes: `__mcount_loc`, `__patchable_function_entries` | equal per function that exists in both, since they depend on inlining decisions |
| the lists objtool writes: `.static_call_sites`, `.retpoline_sites`, `.return_sites`, `.call_sites`, `.ibt_endbr_seal`, `.orc_unwind`, `.orc_unwind_ip` | not counted, and not in the sections an object is compared on either. objtool builds them from the code, an ORC entry for every stack change and a return site for every `ret`, so the counts are each compiler's own instructions. `rk objtool-report` is the check for them. Before IBT, as in 6.1, every object has them, and from then on only `vmlinux.o` does |
| the tables written by the code they describe: `__jump_table`, `__bug_table`, `__ex_table`, `.altinstructions` | the same set of distinct sites per object. Every inlined copy of a `static_branch_unlikely` or a `WARN_ON` adds an entry, so the counts follow the inliner like the call-site lists do. A site is what the entry says apart from code addresses: the key and branch of a jump label, the format, file, line and flags of a bug, the fixup type of an exception entry without its register, and the CPU feature of an alternative |
| `Module.symvers`: exported symbols, their namespaces, and CRCs | equal |
| `System.map` global symbol set | equal, apart from compiler-generated local suffixes |
| `.modinfo` per module | equal |
| `__ksymtab_strings` | equal |

Because the kernel's tables are built by C and asm macros that emit to named sections, every failure here is a precise pointer:
- a `section` attribute ignored;
- an `asm goto` dropped;
- a template's `.pushsection` lost;
- an initcall discarded as unused.

**M in the harness, K2.** It runs before any boot and is the main gate between K2 and K3.

## 9.5 vDSO

The x86-64 vDSO (`arch/x86/entry/vdso/`) is a small shared object, built with the kernel's compiler:
- flags `-fPIC -fno-stack-protector -fno-omit-frame-pointer -mcmodel=small -fno-jump-tables -mno-red-zone`, and in 6.x `-fcf-protection` handling;
- linked with `-shared --hash-style=both --build-id=sha1 -Bsymbolic`, with a version script;
- checked by `vdso2c` and by `checkundef` for any undefined symbols. A compiler-invented `memcpy` or `__stack_chk_fail` call fails it.

The 32-bit vDSO (`vdso32`) is built `-m32` for `IA32_EMULATION`, on in x86-64 `defconfig`, so x86-64 needs rucc's i686 target (K4) for an identical configuration. The bring-up flag `-frucc-kernel-bringup-m16` (document 00) also covers `-m32` for the vDSO until K4. Builds using it are not graded.

arm64 has `vdso` (and `vdso32` with `COMPAT_VDSO`, which needs an arm32 compiler, so it is `n` without `CROSS_COMPILE_COMPAT` on both sides). It is built `-fPIC` with the arm64 rules. The standard `-shared` link is done by `$(LD)`.

## 9.6 Boot images

| Arch | Image | Compiler involvement |
|---|---|---|
| x86 | `bzImage` = setup (`arch/x86/boot/`, 16-bit real mode C built `-m16`) + compressed `vmlinux.bin.gz` wrapped by a decompressor (`arch/x86/boot/compressed/`, 64-bit PIE plus 32-bit entry `head_64.S` `.code32`) | `-m16`/`.code16gcc` (K4), `.code32` assembly (K4), the PIE decompressor with `-fPIE -mcmodel=small` and hidden visibility, EFI stub (`drivers/firmware/efi/libstub/`), built with `-fPIC -fno-stack-protector -mno-red-zone` and its own no-relocation checks |
| x86 | PVH entry (`arch/x86/platform/pvh/head.S`), used by `qemu -kernel vmlinux` | an ELF note (`XEN_ELFNOTE_PHYS32_ENTRY`), so it needs `@note`, and a `.code32` entry (K4 for the assembler part) |
| arm64 | `Image` = vmlinux in raw binary, with the header in `head.S`. `Image.gz` optional | nothing beyond the kernel. The EFI stub is built into the image with `-fpie` and relocations checked by `libstub`'s `stub-check` |
| riscv | `Image` | as arm64 |
| i386 | `bzImage` like x86-64 without the 64-bit decompressor | K4 |

The boot path used for grading x86-64 before K4 is `qemu -kernel bzImage` with the real mode setup built by the bring-up delegation. That boot is not graded. The graded path from K4 is the same image, built entirely by rucc.

Before K4 there is an alternative that needs no 16- or 32-bit code:
- boot `vmlinux` directly through PVH (`qemu -kernel vmlinux` with `CONFIG_PVH=y`);
- `head_64.S` and the PVH entry are `.code32` only in small parts.

But `CONFIG_PVH` is not in `defconfig`, and turning it on is a configuration change that the test fragment may make, because it only turns things on. With PVH on and the vDSO32 question aside, K3 could reach a boot with no delegation except `IA32_EMULATION`'s vDSO. We take this path at K3 and record it in `configs/test.fragment`. Document 11.1 lists it.

## 9.7 Modules

- **Loading.** Modules built by rucc must load into the rucc kernel, and, as the cross-module check, into the GCC kernel and vice versa.
  - Load requires matching `vermagic` (from `UTS_RELEASE` and config, so equal) and matching CRCs under `MODVERSIONS`, which is 06.8.
  - The module's relocations must be of types the kernel's module loader handles:
    - `apply_relocate_add` on x86-64 handles `R_X86_64_64`, `32`, `32S`, `PC32`, `PLT32`, `PC64`, and rejects `GOTPCREL` family relocations before 5.x (later kernels handle `REX_GOTPCRELX` by relaxation, check);
    - on arm64 the list is in `module.c`, and some ADRP forms need PLT veneers the loader builds.
  - `rk modules-audit` lists relocation types per module and fails on anything the loader for that version rejects.
- **`rk cross-modules`** (K5). Boot the GCC kernel, load rucc-built modules from `allmodconfig` (those that load in QEMU without hardware: about 400 of them, the list computed by trying with GCC's modules first), run their KUnit suites, and swap. That proves ABI compatibility at the module boundary, which is the same as the compat corpus's proof for user programs.
- **Module signing** (`MODULE_SIG`) is a host tool step (`sign-file` with OpenSSL), with no compiler involvement.

## 9.8 Debug information

The graded Current configs have `DEBUG_INFO_DWARF5` or `DEBUG_INFO_DWARF_TOOLCHAIN_DEFAULT` with `DEBUG_INFO_BTF` in the distro config. So:
- rucc's DWARF 5 must satisfy `pahole` (1.25+). pahole reads DWARF with `libdw`; types, members, bitfield offsets, enums and function prototypes all matter.
- **The BTF differential:** `bpftool btf dump file vmlinux format raw` from both builds, normalized for IDs and sorted, must describe the same types and the same functions. Functions only present in one because of inlining are listed and not failed.
- `DEBUG_INFO_DWARF4` needs `-gdwarf-4` (refused today; M8 in rucc's spec covers DWARF, issue #9). It is **M** to emit DWARF 4 forms from the same generator, and it is the default on E8 and E9 era distribution configs.
- `-gz=zlib` (`DEBUG_INFO_COMPRESSED`) is a no-op in rucc today. Either implement it or refuse the flag so the probe fails honestly. As it stands the probe says yes and nothing happens, which changes nothing in the code, but `.config` says compressed and the sections are not. Refusing is **S** and correct until implemented.
- `-gsplit-dwarf` (`DEBUG_INFO_SPLIT`): refused. Equal `.config` requires the reference to agree, and it will not, so a distro config enabling it is compile-only. Debian doesn't, check Fedora.
