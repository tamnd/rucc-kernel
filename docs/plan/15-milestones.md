# Milestones

Thirteen milestones, K0 to K12. Each one gives:
- its scope;
- the work in each repository;
- an exit criterion that a command in `rucc-kernel` checks;
- a cost in engineer-weeks.

Costs assume one engineer who knows rucc, working full time, and they include the reductions and corpus cases each fix brings. They are estimates made before anything has run, and K0's first job is to make them less wrong. The objtool item (K3) and the macro assembler (K2) carry the most uncertainty. Open questions 2 and 5 are about them.

The milestones are sequential on the critical path, but not everything in a milestone is. Where work can start early, the text says so. Nothing counts as done until its exit criterion holds on one rucc commit in one nightly run, with no bring-up flag, as 02.7 requires.

## Summary

| | Scope | Rows | Cost | Critical path through |
|---|---|---|---|---|
| K0 | the instrument: repo, pins, eras, baselines, probes, demands, `kernel-pp` | X64, A64 baselines | 4 weeks | era containers, `rk probes` |
| K1 | identity and front end: `.config` identical, every unit passes `-fsyntax-only` | X64 | 4 weeks | persona banner, `-Wa`, ICE fixes, `section` |
| K2 | objects and link: x86-64 `vmlinux` links, section differential clean (bring-up flags allowed) | X64 | 12 weeks | macro assembler, `asm goto`, code model, general-regs-only |
| K3 | first graded boot: x86-64 `defconfig` and `tinyconfig` boot, KUnit parity, objtool clean | X64 | 6 weeks | objtool, mitigations in `defconfig`, `lkmm` |
| K4 | i686: `-m16`, `-m32`, `i386_defconfig` boots, no delegation anywhere | X64, X32 | 8 weeks | the i686 back end, `.code16` |
| K5 | x86-64 hardening, distro config, `allmodconfig`, kselftest, LTP, cross-modules | X64 | 10 weeks | hardening flags, DWARF/BTF, CRCs |
| K6 | arm64 at K5's level | A64 | 6 weeks | sysregs, PAC/BTI, sp_el0 canary |
| K7 | riscv64 boots `defconfig` with KUnit parity | R64 | 12 weeks (after parent M9) | relaxation |
| K8 | the performance gate | X64 | 10 weeks | inlining, switch lowering, prologues |
| K9 | every longterm and CIP head, Current complete | X64, A64, X32 | 5 weeks | backports of old-kernel constructs |
| K10 | every release from 2.6.12, and every last point | X64, X32, A64 | 10 weeks | era personas, gnu89, old asm |
| K11 | museum: 1.0 to 2.4 boot on i386 | X32 | 8 weeks | old GNU C, a.out, `-traditional` |
| K12 | steady state: HOSTCC=rucc, rc tracking, upkeep | all | 3 weeks, then upkeep | nothing new |

K7 does not count on the critical path. It waits on rucc's parent M9 for a back end, and it runs in parallel with everything from K5 on.

Total: about 86 weeks for one engineer (K0 to K6 and K8 to K12), plus K7's 12 weeks. Allowing 25% for the usual underestimate on the two uncertain items, **about two engineer-years**. With two engineers it is about 14 months, following the tracks at the end of this document.


## K0: the instrument

**Scope.** Nothing in the kernel is expected to build yet. K0 builds the tools, fixes the pins and eras, records the baselines, and measures the gap, so that every later cost is based on a census instead of a guess.

**rucc.**
- Open the issues listed in 14.1. Fix the stale metadata of 03.8.
- Propose the spec edits of 14.1: i686 as M10's fourth target, M11's levels mapped to K3, K5 and K8, and `-O2`/`-Os` only for the kernel.
- `rucc --print-features`, the machine-readable support table `rk demands` consumes (S).
- Static musl build of rucc checked in CI, since the era containers need it (S).

**rucc-kernel.**
- The repository with the layout of 13.2.
- `rk fetch`, `rk sets` with `sets.toml` expanded into `pins.toml`, and `personas.toml` with every era of 04.2.
- Era containers E3 to E11 built and pushed, with E0 to E2 recipes started.
- `rk personas check` on E9 to E11.
- `rk baseline` for 7.2.x, 6.12.y and 6.1.y `defconfig` on X64 and A64, three runs, with boot, smoke and KUnit.
- The shim, `rk probes`, `rk demands`, `rk asm-inventory`, `rk boot` (the rig and `rk-init`), the initramfs.
- Provisioning for gpc, server3, server2 and server1, and gpc added to `test-machines.md`.

**rucc-compat.** The `kernel-pp` and `kernel-probes` corpora for 7.2.x.

**Exit criterion.**
1. `rk baseline` has run for the three versions on X64 and A64, and G is committed for each. The times in 13.6 are replaced by measurements.
2. `rk probes 7.2.x --row X64` runs every probe with both compilers and writes the table of differing answers, whatever it says.
3. `rk demands 7.2.x` has run on X64 and A64 with `defconfig` and `allmodconfig`. Every demanded feature is either present in rucc or has an open issue, and the census table (blocked units per missing feature) is in `reports/`.
4. `rk asm-inventory 7.2.x` output is committed, and every missing mnemonic class has an issue.
5. The `kernel-pp` corpus shows no unexplained difference for x86-64 `defconfig`.

**Cost.** 4 weeks:
- 2 for the harness and rig;
- 1 for containers and baselines;
- 1 for the census and the issues it produces.

## K1: identity and front end

**Scope.** On x86-64, for 7.2.x, 6.12.y and 6.1.y `defconfig`:
- kbuild accepts rucc;
- `.config` is identical to the reference's;
- every C and `.S` unit gets through rucc's front end.

This is the milestone where the kernel stops failing in the configuration step and starts failing in the compiler.

**rucc.**
- The persona of 04.2: banner, default std, `-fcommon` for old personas, `-dumpfullversion`, `-print-file-name=plugin`.
- `-Wa,` allow-list, `-Wa,--version` with the GNU assembler banner, `-fgnu-as-version=`.
- The no-op and one-line flags table (03.2, 05.7), the full GCC `-W` names list, and target by program name.
- Null pointer constant ICE (06.2), `const` ICE (06.3), `__has_attribute` accuracy.
- The `section` attribute (#909) and `error`/`warning` attributes. Both are needed by K2, and they are cheap enough to do now.
- `.S` preprocessing (07.1).
- Probe-affecting refusals, where a real implementation is K2 work, made into honest refusals. For example `-mcmodel=kernel` stays refused with a clear message.

**rucc-kernel.** `rk config-diff` with `--why`, `rk flags-diff`, `rk syntax` (compile every unit with `-fsyntax-only` for C and `-E` for `.S`, using the reference build's `.cmd` lines with the compiler swapped).

**rucc-corpus.** `null-pointer-constant`, `const-ice`, `section-attr`.

**Exit criterion.**
1. `rk config-diff 7.2.x --row X64 --config defconfig` reports no difference beyond `config-divergences.toml` with the rucc persona, when the compiler features are asked honestly. The probes that depend on K2 work (the stack protector probe, `CC_HAS_RETURN_THUNK`, the mitigations) will differ. They are listed as expected differences with the K2 issue attached, **in the run report, not in `config-divergences.toml`**. So K1's criterion is exactly this: every remaining difference is a probe whose flag has an open K2 issue.
2. `rk flags-diff` shows the same, and no other difference.
3. `rk syntax 7.2.x --row X64` passes every C unit and every `.S` unit's preprocessing for `defconfig`, and for `allmodconfig` apart from a list whose every entry has an issue.
4. The same on 6.12.y and 6.1.y.

**Cost.** 4 weeks. The persona and flag tables are 1, the ICE fixes and `section` 1, `.S` preprocessing and the syntax pass fixes 2.

## K2: objects and the link

**Scope.** Every unit of x86-64 `defconfig` compiles and assembles with rucc, and `vmlinux` links. The section, symvers and vec-audit differentials are clean. Bring-up flags are allowed for `-m16` and `-m32` only (the i686 parts, K4). Nothing is booted and graded yet.

**rucc.** The critical path of 03.9:
- **Assembler (07.7 items 2 to 7):**
  - expressions and relaxation;
  - the macro processor;
  - one assembler stream per object;
  - `asm goto` with bodies and outputs (#221);
  - x86 system instructions and operands from the inventory;
  - relocations, notes, groups, empty sections.
- **Codegen:**
  - the kernel code model and non-PIC with `R_X86_64_32S` (08.2);
  - weak symbols without GOT;
  - general-regs-only (08.3);
  - 8-byte stack alignment (08.4);
  - `-mstack-protector-guard-reg`/`-symbol` on x86-64 (08.5, moved forward from K5);
  - retpoline, rethunk, SLS, IBT jump table rule (08.6, moved forward);
  - `-fno-jump-tables`.
- **Front end:** the `__builtin_constant_p` contract (06.4), global register variables, the missing inline asm modifiers.

This work can start during K1. The macro processor and the per-object stream depend on nothing in K1.

**rucc-kernel.** `rk sections-diff`, `rk symvers-diff`, `rk vec-audit`, `rk modules-audit`. The shim's `RK_BRINGUP` path.

**rucc-compat.** `kernel-asm` graded (07.6).

**rucc-corpus.** `constant-p-after-inline`, `asm-goto`, `asm-local-labels`, `gas-macros`, `mcmodel-kernel`, `general-regs-only`.

**Exit criterion.**
1. `rk build 7.2.x --row X64 --config defconfig` with `RK_BRINGUP=m16` produces `vmlinux` and `bzImage`.
2. `rk config-diff` and `rk flags-diff` are clean. The K1 expected differences are gone, because the flags now exist and work.
3. `rk sections-diff` and `rk symvers-diff` against the reference build: no difference outside the rules of 09.4.
4. `rk vec-audit`: zero vector or x87 instructions in no-FPU objects.
5. The `kernel-asm` corpus: every `.S` unit and every GCC `-S` output of `defconfig` assembles byte-identically, or has a recorded explanation.
6. The same for 6.12.y and 6.1.y.
7. `RK_TWICE=1`: every object identical across two compiles.

**Cost.** 12 weeks:
- assembler 7 (macro processor 3, per-object stream and labels 2, `asm goto` 1, instruction tables 1);
- codegen 4 (code model and non-PIC 2, general-regs-only 1, mitigations 1);
- builtin contract and the rest 1.

This is the milestone most likely to overrun, and K0's census should re-estimate it.

## K3: first graded boot

**Scope.** x86-64 7.2.x, 6.12.y and 6.1.y, booted through PVH (09.6) straight into `vmlinux`. `defconfig` has `IA32_EMULATION=y`, so it needs the `-m32` vDSO, and that waits for K4. The test fragment may not turn `IA32_EMULATION` off. So:
- **The graded exit of K3 is `tinyconfig` plus the test fragment.** That configuration has no 32-bit code. With KUnit, virtio and serial console turned on, it is a real kernel, not a toy.
- `defconfig` is built and booted with `RK_BRINGUP=m16`. Those runs are recorded and not graded until K4.

K3 is where the kernel first runs under rucc, and where miscompilations start to show.

**rucc.**
- objtool-driven fixes (09.1): jump table shape, noreturn agreement, frame setup order, stack realignment form, unreachable ends.
- `-mrecord-mcount` and the ftrace sections (08.7).
- Inline small `memcpy`/`memset` (06.5).
- `lkmm` fixes (06.7).
- Frame size work found by `rk frames` (08.8).
- Whatever `rk mixed` finds.

**rucc-kernel.** `rk test` with attribution, `rk mixed`, `rk objtool-report`, `rk frames`, the dmesg subset check. The nightly workflow runs Current X64.

**rucc-corpus.** `objtool-shapes`, `lkmm`, `frame-size`, `mitigations`.

**Exit criterion.**
1. `rk test 7.2.x --row X64 --config tinyconfig+test --kinds boot,smoke,kunit`: every unit in G passes, with no bring-up flag, three nightly runs in a row.
2. The same for 6.12.y and 6.1.y.
3. objtool: zero warnings for those builds, and for the `defconfig` build made with `RK_BRINGUP=m16`.
4. `rk frames`: no function over `FRAME_WARN` that GCC keeps under it.
5. `rk mixed` has bisected at least one real failure to a function end to end (proof that the tool works). If there were no failures, a deliberately injected one via `-fpass-fuel-global` is used.
6. `rk test ... --config defconfig` with `RK_BRINGUP=m16` passes G, recorded and ungraded.

**Cost.** 6 weeks. objtool 3, the rest 3. If objtool turns out to need upstream conversations (open question 2), this becomes 10.

## K4: i686

**Scope.** rucc gains the i686 target, and everything x86 builds without delegation:
- the x86-64 `bzImage` real mode setup and the 32-bit vDSO;
- `i386_defconfig` for X32.

**rucc.** The i686 back end (10.3):
- `regparm`;
- 64-bit arithmetic without introduced libgcc calls;
- `-march` subsets;
- `R_386_*`;
- non-PIC default;
- the `%fs` stack protector.

In the assembler, `.code16gcc` prefix insertion, `.code16` and `.code32` encoding, and 16-bit addressing forms.

This can start in parallel with K3 by a second engineer. The instruction tables are shared with x86-64.

**rucc-kernel.** The X32 row, and `rows.toml` entries for `qemu-system-i386`. Baselines for 7.2.x, 6.12.y and 6.1.y `i386_defconfig`.

**rucc-corpus.** `i386-kernel`.

**Exit criterion.**
1. `rk test 7.2.x --row X64 --config defconfig+test --kinds boot,smoke,kunit`: G passes, **no bring-up flag in any unit**, booted from `bzImage` (not only PVH). Three nightlies.
2. `rk test 7.2.x --row X32 --config defconfig+test --kinds boot,smoke,kunit`: G passes.
3. `rk config-diff` clean for both. objtool clean for both (i386 objtool runs from 5.x on).
4. The same on 6.12.y and 6.1.y.
5. The `RK_BRINGUP` code path in the shim is still there but has not been used by any nightly for a week.

**Cost.** 8 weeks: back end 5, assembler modes 2, kernel fixes 1.

**This is the milestone at which M11 level A closes in rucc's spec.** "rucc builds and boots the kernel" is true from here on, without qualification, for x86.

## K5: x86-64 at distribution strength

**Scope.** Everything 02.3 lists for Current X64:
- the distro config (Debian 13, then Fedora);
- `allmodconfig` builds;
- kselftest and LTP;
- cross-module compatibility;
- DWARF and BTF.

**rucc.**
- The hardening flags off in `defconfig` and on in distro configs (08.6): `-fzero-call-used-regs`, `-ftrivial-auto-var-init`, `-fstrict-flex-arrays`, cs-prefix, call depth tracking padding.
- `no_stack_protector` (#811), `__seg_gs` (03.3).
- The code-changing attributes of 03.3.
- DWARF 4 and the BTF/pahole agreement (09.8), `-gz` resolution.
- `SHF_GNU_RETAIN`, COMDAT groups.
- `__builtin_has_attribute`, `__builtin_counted_by_ref`.
- Everything `allmodconfig`'s 25,000 units demand that `defconfig` did not: the AVX-512 crypto assembler, KVM's VMX/SVM instructions, drm's FPU regions with vector code allowed.

**rucc-kernel.** `rk cross-modules`, kselftest and LTP in the initramfs, the distro config pins, the BTF differential.

**rucc-real-corpus.** LTP and kselftest as user projects (14.4).

**Exit criterion.**
1. `rk test 7.2.x --row X64 --config debian-13+test --kinds boot,smoke,kunit,kselftest,ltp`: G passes, and the dmesg subset holds.
2. `rk build 7.2.x --row X64 --config allmodconfig` succeeds, with config, flags, sections and symvers differentials clean and objtool clean.
3. `rk cross-modules 7.2.x --row X64`: rucc modules in the GCC kernel and GCC modules in the rucc kernel load, and their KUnit suites pass.
4. The BTF differential is clean apart from inlining-dependent functions.
5. 1 to 3 hold on 6.12.y and 6.6.y.

**Cost.** 10 weeks. Hardening 3, `allmodconfig` breadth 4, DWARF/BTF 2, kselftest and LTP attribution work 1.

**M11 level B closes here.**

## K6: arm64

**Scope.** A64 at K5's level for Current: `defconfig` and distro config boot with every test kind, and `allmodconfig` builds.

**rucc.**
- The arm64 kernel flags (10.2): general-regs-only, no-pic model, `sp_el0` canary, PAC/BTI, `-mno-outline-atomics`.
- The arm64 system register and instruction tables (07.4.2), generated.
- `#pragma GCC visibility`, `target("+lse")`.
- Whatever `rk mixed` finds, and it will find memory ordering bugs here first, because arm64 is weakly ordered (06.7).

K6 can start as soon as K2's assembler work is done, by a second engineer. The macro processor and per-object stream are shared.

**rucc-kernel.** A64 baselines for the full set, and KVM on arm runners if open question 8 says yes.

**Exit criterion.** K5's criteria 1 to 4 on A64, with Debian 13 arm64's config and `-cpu max` plus `-cpu cortex-a57` boots, for 7.2.x and 6.12.y.

**Cost.** 6 weeks.

## K7: riscv64

**Scope.** R64 `defconfig` boots with KUnit parity, on Current.

**Prerequisite.** rucc's parent M9 delivers a riscv64 back end that passes rucc's own user-program suites. K7 does not start before that, and its cost excludes the back end.

**rucc.** The kernel specifics of 10.4: medany, relaxation correctness, the `tp` register, the `.option` and `.insn` directives, the stack protector through `tp`.

**Exit criterion.** `rk test 7.2.x --row R64 --config defconfig+test --kinds boot,smoke,kunit`: G passes, and `rk config-diff` is clean. The same for 6.12.y.

**Cost.** 12 weeks, of which about 6 is relaxation and linker interaction. The CCC project found this was where its riscv boot went wrong.

## K8: the performance gate

**Scope.** 02.5's targets on X64 7.2.x `defconfig` on server3:
- each benchmark within 5%, and the geomean within 2%;
- `.text` within 5%, and stack high-water mark within 10%;
- `defconfig` build ≥1.5x faster than GCC and `allmodconfig` ≥1.3x;
- peak RSS under 1.5 GB, and the slowest unit under 3x GCC's time.

**rucc.** The loop of 12.6, in the areas of 12.4. No kernel-specific mode.

K8 can start as soon as K3 gives a booting `defconfig`. Measurement and the first fixes overlap K4 and K5. The exit waits for K5, since the graded configuration's mitigations change the numbers.

**rucc-kernel.** `rk bench`, `rk hot-sizes`, `rk slowest`, and server3 provisioned for exclusive benchmark windows.

**Exit criterion.**
1. `rk bench 7.2.x --row X64 --config defconfig+test` on server3: every benchmark's 95% interval upper bound ≤ 1.05, the geomean ≤ 1.02.
2. The static and compile-time measures of 12.3 and 12.5 meet 02.5.
3. `rucc-real-corpus` and `rucc-postgres` show no regression beyond their noise thresholds, across the K8 rucc commits.
4. 1 and 2 still hold on the next stable point release, measured independently.

**Cost.** 10 weeks. Uncertain. rucc's user-program gap was 32x on pcre2 a year ago and 1.26x now, which suggests the machinery exists and the work is heuristics.

**M11 level C closes here.**

## K9: every longterm and CIP head

**Scope.** The whole Current set (04.1.1) green on X64, A64 and X32:
- 6.18.y, 6.12.y, 6.6.y, 6.1.y, 5.15.y, 5.10.y;
- CIP 4.4.y-cip and 4.19.y-cip.

X64 and A64 at K5's level from 5.10; 4.4 and 4.19 at `defconfig` boot plus KUnit (4.19 has none, so smoke).

**rucc.** Constructs older kernels use that newer ones dropped:
- the `-std=gnu89` default under personas E6 to E9;
- older `asm goto` shapes (4.x without outputs);
- older alternative macros (`ALTERNATIVE` without `.nops`);
- objtool of older versions, which is stricter about some shapes and laxer about others;
- the 4.4 and 4.19 stack protector at `%gs:40`, since the guard symbol form doesn't exist there.

**Exit criterion.** `rk sweep --set current` is green on every member for two consecutive nightlies.

**Cost.** 5 weeks.

## K10: every release

**Scope.**
- Every tag from v2.6.12 to the newest release on X64 and X32 (A64 from 3.7): build, boot, smoke, KUnit where it exists.
- Every last point release (04.1.3) on X64 and X32.

**rucc.**
- Era personas E3 to E8 in full: the E7 `cc-name`/`gcc-version.sh` paths, and `compiler-gcc{3,4,5}.h` selection.
- gnu89 `extern inline` semantics under old personas.
- `.subsection` and `.text N` (2.6 lock sections).
- `LOCK_PREFIX` and `.section .smp_locks` forms.
- The 2.6 i386 `fastcall` and `regparm` usage.
- Old kbuild probe forms (`-S` based `cc-option`, `stderr` sensitivity).
- Constructs GCC 3.4 and 4.1 accepted and the kernel used: for example `__attribute__((used))` on non-static functions in unusual positions, and zero-sized `struct {}` initialized with `{ }`.

**rucc-kernel.** Era containers E3 to E8 in use (built at K0). The weekly releases workflow. `rk sweep` scheduling on gpc.

**Exit criterion.** `rk sweep --set releases` and `rk sweep --set last-points` are green on every member, in one weekly and one monthly run with the same rucc commit.

**Cost.** 10 weeks. Assuming 114 + 90 trees fall into about 20 distinct failure causes, each a week or less. The spread is wide: most of 2.6.x is one era for rucc, and the risk is in the oldest 2.6 trees.

## K11: the museum

**Scope.** Linux 1.0, 1.2.13, 2.0.40, 2.2.26 and 2.4.37.11 build and boot on X32 to `rk-init-museum`.

**rucc.**
- Old GNU C behind persona 2.x (06.1): cast as lvalue, conditional and comma as lvalue, multi-line strings, labels at the end of blocks, `__asm__` with register bindings in old forms, old-style `__attribute__` positions.
- `-traditional` preprocessing for `.S`.
- An `a.out` (OMAGIC/ZMAGIC) object writer for 1.x. This depends on whether 1.x can link ELF objects. 1.2 supports building as ELF, and 1.0 does not, which is open question 4.
- The i386 subset that 1.0's assembler dialect used (`as86` for the boot sector is separate: `bin86` is a host tool of the era container, not the compiler under test).

**rucc-kernel.** `rk-init-museum`. Floppy image or initrd boot through `qemu-system-i386 -fda` or `-kernel` (1.x zImage boots with QEMU's Linux loader for old protocols; check per version). Era containers E0 to E2 finished.

**Exit criterion.** `rk sweep --set museum` is green: each tree built by rucc boots to the marker and passes the museum smoke suite, and each built by its reference does too.

**Cost.** 8 weeks. The a.out writer 2, old GNU C 3, the boot rig for pre-2.4 kernels 2, and 1 for things nobody expects.

## K12: steady state

**Scope.** The harness runs itself.
- Nightly Current, weekly Releases, monthly Last points and Museum.
- A new tag is graded on the day it appears. A new rc is built and booted nightly, ungraded.
- A regression is bisected to a rucc commit automatically.
- `HOSTCC=rucc` for X64 Current: the kernel's host tools, including objtool with libelf, `modpost`, `genksyms`, `fixdep` and `kconfig`, compiled by rucc. It is an ungraded column that becomes graded once green for a month.
- The ungraded probes: the newest GCC as a second reference on old trees, `ld.lld` as the linker, and sanitizers when rucc has any.

**Exit criterion.** Four consecutive weeks with every set green, and with each regression that appeared in those weeks bisected and fixed within the week.

**Cost.** 3 weeks to finish automation. After that, upkeep of about 10% of one engineer, rising for each new kernel feature that needs compiler support. The kernel adds one or two a year (`counted_by`, named address spaces, the next hardening flag).


## Parallel tracks

With two engineers:

```
Engineer A:  K0 ── K1 ── K2 (assembler) ────────── K3 ── K5 ── K8 ────── K9 ── K12
Engineer B:        K1 (ICEs) ── K2 (codegen) ── K4 (i686) ── K6 ── K10 ── K11
                                                          K7 when M9 lands (either)
```

- K2's assembler and codegen halves split cleanly.
- K4 needs K2's codegen, not K3's objtool work.
- K6 needs K2's assembler.
- K8 needs K3 for a bootable kernel and K5 for the graded configuration.
- K10 and K11 need K4, because they run on X32.

About 14 months end to end with two engineers. That matches the "two engineer-years" of 01.7, with the overlap.

## Cost table

| | Weeks | Cumulative | Uncertainty |
|---|---|---|---|
| K0 | 4 | 4 | low |
| K1 | 4 | 8 | low |
| K2 | 12 | 20 | **high** (macro assembler, per-object stream) |
| K3 | 6 | 26 | **high** (objtool) |
| K4 | 8 | 34 | medium |
| K5 | 10 | 44 | medium |
| K6 | 6 | 50 | medium |
| K8 | 10 | 60 | medium to high |
| K9 | 5 | 65 | low |
| K10 | 10 | 75 | medium |
| K11 | 8 | 83 | high (a.out, 1.0 boot) |
| K12 | 3 | 86 | low |
| K7 | 12 | (parallel) | high, and gated on M9 |

With 25% contingency on K2, K3, K8 and K11, that is about 95 weeks. That is the two engineer-years stated in 01.7. It is well beyond the parent spec's 4 to 8 months, which buys roughly K0 to K4 (34 weeks): x86 builds and boots, with no hardening, arm64 or old versions.
