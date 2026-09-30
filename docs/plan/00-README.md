# Spec 2131/corpus/kernel: every Linux kernel, unmodified, built by rucc

rucc builds an unmodified Linux kernel, boots it, and the kernel passes the tests that the same kernel passes when the reference GCC builds it from the same `.config`. That holds for every release Linus has tagged since 2.6.12, for the head of every maintained longterm branch, for mainline as it moves, and for a museum set of the historic kernels from 1.0 to 2.4. On x86-64 and AArch64 at current mainline, the kernel it produces is as fast as GCC's, measured with the kernel's own `perf bench` and a fixed set of scheduler, syscall, memory and I/O benchmarks. And rucc builds it faster than GCC does. Every change that makes this true lands in `tamnd/rucc`. The harness, the pins, the boot rig, the bisection tools and the reports live in a new repository, `tamnd/rucc-kernel`. Four existing repositories grow to carry what the kernel teaches: `tamnd/rucc-compat`, `tamnd/rucc-corpus`, `tamnd/rucc-real-corpus` and `tamnd/rucc-cross`.

Written 30 September 2026, against rucc 0.17.0 (commit `df975c11`). The kernel pins are Linux 7.2.8 (stable, 25 September 2026), 7.3-rc5 (mainline, 27 September 2026), and the longterm heads 6.18.54, 6.12.111, 6.6.157, 6.1.188, 5.15.221 and 5.10.270. This folder refines M11 of the parent specification (document 17, issue tamnd/rucc#12), rung 4 of the target ladder (document 14.5), and the kernel half of M10 (tamnd/rucc#11). Where they disagree, this folder is newer and says so.

## Why the kernel is different from everything before it

SQLite showed that rucc can compile one very large, very careful C file, and Postgres showed that it can build a big program with its own build system and pass a hard test suite. The kernel is a different kind of test in four ways.

**The compiler is part of the program.** No user program depends on its compiler the way the kernel does. Here is what the kernel relies on:
- **Sections.** It places code and data in named sections, and the linker script turns them into arrays. That is how initcalls, exception tables, jump labels, alternatives, static calls, tracepoints and exported symbols work.
- **Inline assembly as a language.** It writes self-patching machine code in inline assembly: `ALTERNATIVE`, `asm goto` static branches, and `_ASM_EXTABLE` fixups. It assembles about 1,350 `.S` files through the C compiler, with the full GNU assembler macro language.
- **Optimization for correctness.** It checks at build time that dead code was removed. `BUILD_BUG_ON`, `compiletime_assert` and the FORTIFY checks become link errors or build errors if the optimizer does not fold what GCC folds.
- **A checked code shape.** On x86-64, a tool named `objtool` reads every object file the compiler writes. It rejects code whose stack frames, jump tables, return sequences or calls out of `noinstr` sections it cannot follow.

**The compiler is identified, not probed.** The kernel asks what the compiler is and what version it has, in three different ways over its history. It then turns features on by version number and trusts the answer. A compiler that answers wrongly gets a kernel configured for a different compiler.

**There is no test suite without a machine.** A kernel is tested by booting it. Everything in this folder that says "passes" means a virtual machine started, the kernel reached `init`, something ran, and the machine powered itself off with the right output on the serial console.

**"All versions" is thirty-two years of C.** Linux 1.0 was built by GCC 2.5 as `a.out` objects, and 7.3 needs GCC 8.1 and C11. In between, the kernel used GNU C features that GCC later removed, such as cast-as-lvalue, and relied on the `__GNUC__` value to pick a header file. Every era also has a toolchain that builds it and a newer one that does not.

If rucc builds all of that without a patch, it is a compiler the systems world can use. That is the third stopping point in the parent README.

## Where we start from

Document 03 is the full inventory. The headline follows.

**The C front end is in good shape.** It already handles:
- statement expressions, `typeof` and `typeof_unqual`, case ranges, computed goto and zero-length arrays;
- `__int128`, `naked`, `alias`, `weak`, `cleanup`, `always_inline`;
- `__atomic` and `__sync`, the overflow builtins;
- `-fpatchable-function-entry`, `-mfentry`, `-fcf-protection`, `-mno-red-zone`, `-fstack-usage`;
- kbuild's `-Wp,-MD`. busybox, a kbuild project, builds and passes in `rucc-real-corpus`.

**The code generator is no longer the obstacle it was.** pcre2 at `-O2` took 32 times as long as GCC when the Postgres folder was written. The latest real corpus cost report (rucc 0.15.1) has it at 1.26 times. rucc compiles most projects in half the time GCC takes at `-O2`.

**What stands in the way is everything below the C.** On x86-64 today, rucc cannot compile a single kernel file:
- `-mcmodel=kernel`, `-fno-PIE`, `-mno-sse` and `-mgeneral-regs-only` are refused.
- The assembler has no `.macro`, `.irp`, `.rept` or `.if`.
- There are no system instructions (`lgdt`, `wrmsr`, `swapgs`, `%cr3`) and no `R_X86_64_32S` relocation.
- `asm goto` with a non-empty template is refused.
- `__attribute__((section))` is silently ignored (#909).
- A `const int` in `_Static_assert` is refused. That is a GCC extension, and 7.2's minimum toolchain bump now depends on it.

There is no back end for i386, 32-bit ARM or RISC-V. An x86-64 kernel needs i386 code generation anyway:
- the real mode trampoline and the boot setup code are built with `-m16`;
- the 32-bit vDSO in `defconfig` is built with `-m32`.

## The goal, stated so it can be falsified

Document 02 states each claim precisely. In short:

**Unmodified.** The source tree rucc builds is the pinned upstream tarball or tag, byte for byte. Nothing is patched. Configuration comes only from the kernel's own `make` targets, its in-tree defconfigs and fragments, and a small, published test fragment that is applied identically to the GCC build.

**Configured the same.** After `olddefconfig`, the `.config` rucc produces is identical to GCC's, apart from the lines that record compiler identity. Every `cc-option`, `as-instr` and version gate must answer the same way, so both compilers build the same kernel.

**Passes.** The rucc-built kernel boots in QEMU. It passes every KUnit suite, and on the graded rows every kselftest and LTP case, that the GCC-built kernel passes with the same configuration. Its dmesg shows no warning, oops or sanitizer report that GCC's does not. On x86-64, `objtool` reports nothing.

**All versions.** Four sets, from document 04:
- **Current:** mainline, stable and every longterm head, nightly.
- **Releases:** every non-rc tag from 2.6.12 to today, 114 tags as of this writing, built and booted.
- **Last points:** the last point release of every stable and longterm branch that ever existed.
- **Museum:** 1.0, 1.2, 2.0, 2.2 and 2.4 on i386.

**Very high performance.** Measured at 7.2 on x86-64 in a KVM guest on server3:
- a fixed benchmark set, with each benchmark within 5% of the GCC-built kernel and the geometric mean within 2%;
- `vmlinux` text within 5% of GCC's size;
- a `defconfig` build at least 1.5 times faster than GCC's on the same machine, with no rucc process over 1.5 GB.

**Four architectures.** x86-64 and AArch64 are graded on every set from 5.x onward. i386 is graded on every set, because it is the only architecture the museum kernels have. riscv64 is graded on current and longterm once the RISC-V back end exists. Everything else is compile-only and informational.

## Settled decisions

**The persona is a flag, and the harness sets it per era.** `-fgnuc-version=` already exists in rucc and changes `__GNUC__` and `-dumpversion`. It has to grow to change the `--version` banner and the default C dialect as well, because the kernel reads both (document 05). The harness passes it through `CC`, as in `make CC="rucc -fgnuc-version=4.9.4"`. That is configuration, not a patch: `make CC=` is the kernel's documented interface for choosing a compiler. Which version each era gets is data in `rucc-kernel/personas.toml`, never a guess made inside rucc.

**The reference is a period GCC with period binutils, per era.** A 2.6.32 kernel is graded against GCC 4.4 and binutils 2.20 in a Debian squeeze image, not against GCC 16. A kernel that fails with the period GCC is not in the graded set. Document 04 has the table.

**rucc is the compiler and the assembler. The linker is binutils.** rucc has no linker and needs none here. The kernel calls `$(LD)` itself, with its own linker scripts. GNU ld is the default on every row, and `ld.lld` is graded as a second linker on current kernels only. The assembler is different: the kernel assembles `.S` files and inline assembly through `$(CC)`, and rucc has no external assembler by design. So rucc's integrated assembler has to take the whole kernel assembly language. That is the largest single piece of work in this folder (document 07).

**Bring-up may hand things to GCC, and nothing measured that way counts.** Two bring-up flags exist, both named to be embarrassing:
- `-frucc-kernel-bringup-as=<gas>` sends `.S` files and inline assembly rucc cannot assemble to GNU as.
- `-frucc-kernel-bringup-m16=<gcc>` sends `-m16` and `-m32` units to GCC.

This is how Claude's C Compiler got its first boot. It lets K2 debug code generation before the assembler and the i686 back end are finished. No exit criterion is met by a build that used either flag, and the record says so.

**x86-64 first, AArch64 immediately after, i686 forced, RISC-V when the parent M9 delivers it.** The parent ladder names x86-64 for level A, and the test machines are x86-64 hosts with KVM. AArch64 has a working back end and no 16 or 32 bit side trip, so it runs one milestone behind as a check on the assumptions. i686 is not optional, for two reasons: x86-64 boot needs it, and the museum and most old releases are i386 kernels. It is the "fourth target" M10 left open, and this folder settles M10's choice: i686, not 32-bit ARM.

**`objtool` clean is correctness, not polish.** On x86-64 since 4.6, `objtool` validates every object. Since 4.14 the ORC unwinder it generates is the default unwinder. A warning means one of three things: backtraces will be wrong, `noinstr` code calls instrumentable code, or a speculation mitigation is missing. The parent spec's level A already says no warnings, and this folder keeps it.

**`CONFIG_RUST` is off.** A C compiler cannot build Rust. Every graded configuration sets `CONFIG_RUST=n`, which is the default, and the GCC baseline uses the same setting. The Rust-only drivers (Nova, Tyr, Rust binder) are outside the claim, and document 02 says so.

**Compiler fixes go to rucc, tools go to rucc-kernel, reductions go to the corpora.** `rucc-kernel` contains no compiler code and no kernel patches. Every miscompilation found through the kernel is reduced to a small program, and it lands in `rucc-corpus` or `rucc-compat` with the fix. This is the same rule as `rucc-postgres` (document 13).

**The oracle is GCC's kernel on the same machine, not the kernel's own opinion.** A KUnit case can fail on the GCC-built kernel too. So can a kselftest that needs hardware, or an LTP case that depends on the host. We record the GCC baseline three times per row, per pin and per configuration, and grade against it (document 11).

**Performance is a gate.** K8 cannot close with the benchmark geomean more than 2% behind, or any single benchmark more than 5% behind. Kernel code is branchy, pointer chasing and lock heavy. Those are the shapes where a register allocator and a scheduler show, and they are the shapes the parent spec's code quality axis is about (document 12).

## The documents

| | | |
|---|---|---|
| 00 | this file | the goal, the settled decisions, the reading order |
| 01 | `01-research-2026.md` | kernel releases, eras, toolchain minimums, prior art |
| 02 | `02-the-goal.md` | the claims, stated precisely, and what does not count |
| 03 | `03-where-rucc-stands.md` | the gap inventory, per area, with issue numbers and sizes |
| 04 | `04-versions-and-personas.md` | what "all versions" means, the four version sets, the persona and reference per era |
| 05 | `05-kbuild-and-identity.md` | how the kernel identifies a compiler, Kconfig probes, the `.config` differential |
| 06 | `06-language-and-builtins.md` | the GNU C the kernel uses, era by era, and optimization as correctness |
| 07 | `07-assembler-and-inline-asm.md` | the integrated assembler, `asm goto`, alternatives, the section language |
| 08 | `08-codegen-and-abi.md` | code models, no FPU, stack protector, mitigations, ftrace, the memory model, frames |
| 09 | `09-objtool-linking-and-images.md` | objtool and ORC, linker scripts, vDSO, boot images, modules, BTF |
| 10 | `10-architectures.md` | x86-64, AArch64, i686, riscv64, and the compile-only rest |
| 11 | `11-boot-and-tests.md` | the boot rig, KUnit, kselftest, LTP, the oracle, bisection |
| 12 | `12-performance.md` | the benchmark set, the build clock, the reporting rules |
| 13 | `13-rucc-kernel.md` | the new repository, its commands, its tracing, what it may not contain |
| 14 | `14-sibling-repos.md` | what rucc-compat, rucc-corpus, rucc-real-corpus and rucc-cross gain |
| 15 | `15-milestones.md` | K0 to K12, exit criteria, costs |
| 16 | `16-open-questions.md` | the ranked list, and when each has to be answered |

Read 02, 03 and 15 first, then 07, because the assembler is the critical path. The rest is reference for whoever picks up a milestone.
