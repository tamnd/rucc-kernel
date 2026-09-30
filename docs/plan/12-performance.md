# Performance

"High performance" has two meanings here, and 02.5 grades both:
1. How fast the kernel rucc builds runs.
2. How fast rucc builds it.

This document fixes the benchmarks, the method, and what we expect to have to fix.

## 12.1 Where the time goes in a kernel

Kernel performance is different from user program performance in ways that change which compiler weaknesses matter:
- **Most hot paths are short and branchy.**
  - Syscall entry and exit.
  - Scheduler pick and switch.
  - Page fault handling.
  - Slab allocation.
  - Futex hash lookups.
  - Network and block stack per-packet or per-request code.

  Loop optimization and vectorization matter little. Vectors are forbidden in most of the kernel anyway. What matters is:
  - inlining decisions;
  - register allocation around calls;
  - branch layout guided by `likely`/`unlikely`;
  - tail calls;
  - the cost of the prologue and epilogue under mitigations;
  - how well `switch` is lowered.
- **Mitigations dominate some paths.** With retpoline and rethunk, every indirect call and return costs a thunk. GCC's advantage over a naive compiler is in devirtualizing and in avoiding indirect calls altogether (`static_call` and `INDIRECT_CALL_*` wrappers are the kernel's own work, the same for both).
- **Code size is performance.** The kernel's instruction cache and iTLB footprint is large, so a 10% bigger `.text` costs throughput in syscall-heavy benchmarks even when every function is locally as fast.
- **The compiler must not add work to the fastest paths.** Examples: an unnecessary frame on a leaf function, a spilled register in `__schedule`, or a stack protector check in a function GCC's heuristic leaves unprotected.

rucc's current code quality evidence is from user programs: pcre2 at 1.26x GCC's time at `-O2` after #1547, and the Postgres folder's numbers. Nothing is known about kernel shapes. K8's first task is measurement.

## 12.2 The fixed benchmark set

Run in a KVM guest on server3. Details:
- **Guest:** 8 vCPUs pinned 1:1, host `performance` governor, SMT off, turbo off, host otherwise idle.
- **Kernel:** 7.2.x `defconfig` plus the test fragment, at `-O2`.
- **Userland:** the same initramfs, with the benchmark tools built by GCC. That way userland is the same for both kernels, and only the kernel differs.
- **Runs:** each benchmark 11 times, alternating kernels (rucc, GCC, rucc, ...), rebooting between runs. We report the median and the interquartile range.

| # | Benchmark | What it exercises |
|---|---|---|
| B1 | `perf bench sched messaging -g 20 -l 1000` (hackbench) | scheduler, pipes/sockets, wakeups |
| B2 | `perf bench sched pipe -l 1000000` | context switch round trip |
| B3 | `perf bench syscall basic` (getppid loop) | syscall entry/exit, mitigations |
| B4 | `perf bench futex hash`, `wake`, `lock-pi` | futex hashing and wakeups |
| B5 | `perf bench mem memcpy`, `memset` (user-space copies, so no kernel code) | control: must be equal |
| B6 | will-it-scale `page_fault1`, `page_fault2`, `mmap1`, `brk1`, `open1`, `read1`, `lock1`, `signal1`, `getppid1`, `context_switch1` (process mode, 1 and 8 tasks) | per-operation kernel paths at scale |
| B7 | lmbench `lat_syscall null/read/write/stat/open`, `lat_ctx`, `lat_pagefault`, `lat_proc fork/exec`, `bw_pipe`, `bw_unix` | classic micro latencies |
| B8 | `fio` against `null_blk` (`bs=4k`, `randread`, `iodepth=64`, `io_uring` and `libaio`, 1 and 8 jobs) | block layer and io_uring submission |
| B9 | `netperf` `TCP_RR`, `UDP_RR`, `TCP_STREAM` over loopback | network stack |
| B10 | kernel build inside the guest (`make -j8 tinyconfig` of 7.2 with GCC in the guest, from tmpfs) | mixed real workload: fork/exec, page cache, VFS |
| B11 | `stress-ng --class cpu,memory,os --metrics-brief` subset (fixed list) | broad coverage, a guard against unexpected regressions |
| B12 | boot time from the kernel's own timestamps to `RK-BOOTED` | initcalls |

B5 is a control. It measures user code only, so a difference exposes a noisy host.

Results live in `baselines/perf/<machine>/<kernel version>/` (GCC reference) and `runs/<id>/perf.json`. The report shows each ratio with a confidence interval. A benchmark is "within 5%" when the upper bound of the 95% interval of the rucc/GCC median ratio is at most 1.05.

## 12.3 Static measures

These are computed on every Current build, without booting:

| Measure | Target (02.5) | Tool |
|---|---|---|
| `vmlinux` `.text` size | at most 1.05x | `size` |
| module text total (`allmodconfig`) | reported | `size` over `*.ko` |
| per-function size for the 500 hottest functions (from `perf record` profiles of B1 to B9 on the GCC kernel) | reported, sorted by the product of size ratio and hotness | `rk hot-sizes` |
| per-function frame size | at most 1.10x on the high-water mark (runtime); per-function reported | `rk frames` (08.8) |
| number of indirect calls and jumps (retpoline thunk call sites) | at most GCC's | `.retpoline_sites` count |
| number of functions with a stack protector check | reported, should match GCC's heuristic | pattern count |
| number of calls to `memcpy`/`memset` | at most GCC's + 5% | relocation count |

`rk hot-sizes` is the main triage tool. A hot function that grew by 30% is almost always a missed inline, a worse `switch` lowering or spill code, and it can be read by eye.

## 12.4 Expected problem areas

These are ranked by our prior on how much each will cost, before measurement:
1. **Inlining heuristics.** The kernel has many `static inline` functions and relies on GCC inlining small ones, and on it *not* inlining large ones, for both `.text` size and stack use. rucc's inliner was tuned on user programs.
   - GCC honors `-fconserve-stack`, `inline` as a hint, `__always_inline`, and `-finline-limit`-like params.
   - We expect to tune rucc's cost model on kernel profiles, and to add a kernel-specific profile only if tuning the general one fails. The rucc spec prefers one heuristic.
2. **Switch lowering under `-fno-jump-tables`.** Retpoline and IBT builds forbid jump tables. GCC then emits balanced binary decision trees with bit tests. A linear chain of comparisons would be a large regression in the ioctl handlers, `x86_emulate_insn` and the netlink attribute parsers. Syscall dispatch is a `switch` from 6.9 (`x64_sys_call`, generated to avoid the indirect call), so it is on this list for Current kernels.
3. **Prologue and epilogue cost.** Frame pointers, stack protector, `zero-call-used-regs`, `endbr64`, return thunks. GCC shrink-wraps the prologue past early-exit paths (`if (!ptr) return;`), and rucc's machine passes (#1178) may not.
4. **Branch layout.** `likely()`/`unlikely()` must put unlikely code out of line, in the function's cold tail or in `.text.unlikely`. The kernel's fast paths depend on the likely path falling through.
5. **Register allocation around calls** in functions like `__schedule`, `do_syscall_64`, `handle_mm_fault`, `kmem_cache_alloc`: callee-saved register use versus spills.
6. **`memcpy`/`memset` expansion.** Small constant-size copies must be inline moves (06.5), not calls. Otherwise each costs a call plus the `rep movsb` ERMS path, which is slower for small sizes.
7. **`__builtin_constant_p`-driven specialization.** If rucc answers "not constant" where GCC answers "constant" after inlining (06.4), `kmalloc` and `copy_*_user` take the general path. That is correct and slower.

## 12.5 Compile speed

| Measure | Target | Method |
|---|---|---|
| x86-64 `defconfig` build | ≥1.5x faster than GCC | `make -j8` on server3, after `make defconfig` and a warm page cache, 5 runs, median. Both with the same binutils `ld` and gas for GCC. rucc's assembler is integrated, so its time includes assembling |
| x86-64 `allmodconfig` build | ≥1.3x faster | same, `-j8`, one run nightly, 3 for a graded number |
| peak RSS of any rucc process | < 1.5 GB | the shim records `getrusage` per process |
| slowest unit | < 3x GCC's time on that unit | per-unit times from the shim |

Known risks:
- **Generated giant functions:** `lib/crc*` tables are data, not functions. But `arch/x86/kvm/emulate.c`, `drivers/gpu/drm/amd/display/dc/dml/**` (large floating-point functions with FPU enabled), `lib/zstd/`, `fs/unicode/utf8data`, `crypto/*` unrolled rounds, `lib/crypto/curve25519-*.c`, and the `sound/pci/hda` quirk tables are the shapes that stress an optimizer. The jtckdint case (#1957) is the known pathology. `rk slowest` lists the top 50 units by ratio each night.
- **The integrated assembler on huge `.S` files:** the AVX-512 crypto files are 10,000+ lines after macro expansion. rucc's macro processor must not be quadratic.
- **Per-process startup:** kbuild spawns about 35,000 compiler processes for `allmodconfig` and several hundred thousand `cc-option` probes over a sweep. rucc's startup time (static binary, no dynamic loading) matters at that scale. Measure it, and target less than 5 ms.

## 12.6 How performance work is organized

K8 is a loop:
1. Measure.
2. Take the benchmark with the worst ratio.
3. Profile both kernels with `perf record -a` in the guest.
4. Rank functions by (rucc cycles − GCC cycles).
5. Read the top function's code in both.
6. Name the compiler cause.
7. File an issue with a rucc-corpus test that pins the better code shape, where it can be expressed as a size or pattern check.
8. Fix, re-measure, repeat until 02.5's targets hold.

The one rule: **no kernel-specific switches**. rucc has no "if compiling the kernel" mode. A heuristic change must be justified on the kernel and must not regress `rucc-real-corpus` or `rucc-postgres` by more than their own noise thresholds.
