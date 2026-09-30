# The `tamnd/rucc-kernel` repository

`rucc-kernel` pins kernel trees, reference toolchains and configurations. It builds with rucc and the reference, boots, tests, compares and reports. It has the same shape and the same restrictions as `rucc-postgres`, and whoever knows one should find their way around the other.

## 13.1 What it may and may not contain

The same rules as `rucc-postgres`, stated here for the kernel:
- **No compiler code.** A fix to rucc goes to `tamnd/rucc`. The harness may detect, measure and bisect. It may not work around anything.
- **No kernel patches, and no copied kernel files.** Trees are fetched by hash. The harness never writes into a tree, apart from kbuild's own output under `O=`.
- **No reductions.** A reduced reproducer goes to `rucc-corpus`, or to `rucc-compat` if it is a differential. `rucc-kernel` links to it by issue number.
- **No hidden configuration.** Everything that influences a graded build is in a committed file: pins, personas, toolchains, configs, divergences, exclusions.

## 13.2 Layout

```
rucc-kernel/
  Cargo.toml                 workspace
  crates/
    rk/                      the CLI
    rk-shim/                 rk-cc: wrapper recording every compiler invocation (13.4)
    rk-pins/                 pins.toml, personas.toml, toolchains.toml parsing and fetch
    rk-kconfig/              .config parsing, config-diff, --why
    rk-flags/                .cmd file parsing, flags-diff
    rk-elf/                  sections-diff, symvers-diff, vec-audit, modules-audit, frames
    rk-qemu/                 boot rig, serial framing, 9p results
    rk-tap/                  KUnit/kselftest TAP parser, LTP result parser
    rk-mixed/                mixed-object bisection
    rk-bench/                benchmark driver and statistics
    rk-report/               markdown and JSON reports
  initramfs/
    rk-init/                 PID 1 (C, built by rucc and by gcc)
    rk-init-museum/          1.0-era init
    build.sh                 cpio assembly from pinned busybox, selftests, LTP
  pins.toml                  trees (04.5)
  sets.toml                  set rules (04.1); `rk sets` expands into pins.toml
  personas.toml              eras (04.3)
  toolchains.toml            container digests, QEMU version, binutils per era
  rows.toml                  X64/A64/X32/R64: arch, qemu machine, cpu, console, timeouts
  configs/
    test.fragment.E11        per era
    distro/debian-13-amd64.config    pinned copies of distribution configs (these are distro files, not kernel files)
    distro/fedora-44-x86_64.config
  config-divergences.toml    05.2
  exclusions.toml            units excluded with the reference's failure log
  demands.toml               feature demand census (13.5)
  baselines/
    <row>/<version>/<config>/ reference results, three runs: units, dmesg, sections, sizes, frames
    perf/<machine>/<version>/
  provision/
    eras/<id>/Dockerfile     era containers (04.4)
    machines/                server1..3, gpc setup: KVM, hugepages, CPU pinning, disk layout
  runs/<id>/                 one directory per run: manifest, logs, results (pruned; 13.7)
  reports/
    nightly.md
    weekly-releases.md
    monthly.md
  .github/workflows/
    nightly.yml              Current, all rows, on self-hosted server3/gpc
    releases.yml             weekly sweep
    monthly.yml              last points + museum
    pr.yml                   harness tests only (no kernel builds)
```

## 13.3 Commands

| Command | Does |
|---|---|
| `rk fetch [--set S]` | fetch and verify trees into the cache |
| `rk sets` | expand `sets.toml` into `pins.toml`, show the diff |
| `rk personas check` | build each era's first and last version with the reference; verify the era rules |
| `rk probes V [--row R]` | run every kbuild probe for V against reference and rucc, without building (05.3) |
| `rk baseline V --row R --config C` | build and test with the reference three times; write `baselines/` |
| `rk build V --row R --config C [--cc rucc\|ref] [--keep]` | build one kernel through the shim |
| `rk config-diff V --row R --config C [--why]` | 05.2 |
| `rk flags-diff V ...` | 05.6 |
| `rk sections-diff`, `rk symvers-diff` | 09.4 |
| `rk vec-audit` | 08.3 |
| `rk modules-audit` | 09.7 |
| `rk objtool-report` | bucket objtool output by message and function shape (09.1) |
| `rk frames` | 08.8 |
| `rk boot V ... --suite S` | one boot (11.3) |
| `rk test V ... [--kinds kunit,kselftest,ltp,smoke]` | the graded test run with attribution (11.7) |
| `rk mixed V ... --unit U` | bisection (11.8) |
| `rk cross-modules V ...` | 09.7 |
| `rk bench V ... [--bench B]` | 12.2 |
| `rk hot-sizes`, `rk slowest` | 12.3, 12.5 |
| `rk demands V` | feature census (13.5) |
| `rk asm-inventory V` | mnemonic and directive inventory from reference `.s` (07.4) |
| `rk sweep --set S` | run a set, with job scheduling across machines |
| `rk repro <run-id> <unit>` | print the exact commands to reproduce one failure by hand, without `rk` |
| `rk report` | write `reports/*.md` from `runs/` |
| `rk nightly` | fetch, sets, sweep Current, report |

Every graded command takes `--rucc <path>` and records the binary's hash and `rucc --version` in the run manifest. It refuses to run if the binary was built with uncommitted changes (rucc embeds a dirty flag, check) unless `--ungraded` is passed.

## 13.4 The shim

`rk-cc` is placed in `CC` as `rk-cc -- rucc -fgnuc-version=...`. It:
- runs the real compiler with the arguments unchanged;
- records, in `compile.jsonl`, one line per invocation: cwd, argv, input and output paths, exit status, wall and CPU time, peak RSS, stderr hash, and whether the call was a probe (output to `/dev/null` or a temp file) or a real unit;
- with `RK_TWICE=1`, compiles every real unit twice and compares the object bytes. A difference is a determinism failure and is written to the run's `nondeterminism.jsonl` (02.7);
- with `RK_BRINGUP=...`, applies the bring-up delegation of document 00 and marks the run ungraded. That is the only place the bring-up flags can be introduced, so it is never done by accident;
- never changes arguments in a graded run.

The same shim wraps the reference `gcc`, and gas through `-Wa,--version` passthrough, so timing and probe logs are comparable.

## 13.5 The demand census (`rk demands`)

`demands.toml` records, per version and row, which compiler features the build actually uses. It is computed from the reference build's `-E` output, `.s` output and `.cmd` files:
- which attributes appear, and how often;
- which builtins;
- `asm goto` count, and `asm goto` with outputs;
- inline asm constraint letters and modifiers used;
- assembler directives and mnemonics (from `rk asm-inventory`);
- `-f`/`-m` flags passed;
- section names created.

Each feature carries rucc's support state from a table rucc publishes (`rucc --print-features`, to add, S). The census then says, per version, "these 14 features are missing, blocking 3,112 of 3,410 units", which ranks the work. It is the kernel version of `rpg`'s demand file and the first thing K0 produces. It regenerates when a new tag is pinned, so a new kernel's new demand shows up the day it is released.

## 13.6 Machines and jobs

| Machine | Role |
|---|---|
| gpc (32 cores x86-64) | the sweep worker: Releases and Last points builds, `allmodconfig`, era containers. KVM for X64 and X32 boots |
| server3 (8 cores x86-64) | the benchmark box (12.2). Nightly Current X64 tests when not benchmarking. Benchmarks take exclusive use |
| server2 (6 cores), server1 (4 cores) | TCG boots for A64 and R64. Nightly A64 KUnit and kselftest |
| GitHub `ubuntu-24.04-arm` runners | A64 boots under KVM if available (open question 8) |
| GitHub `ubuntu-24.04` runners | `pr.yml` harness tests. Small `tinyconfig` smoke builds as a canary |

gpc is not yet listed in `test-machines.md`. K0 adds it with its provisioning file.

The scheduler in `rk sweep` assigns jobs by row and resource: builds need cores and disk, KVM boots need `/dev/kvm`, benchmarks need exclusivity. Jobs are sent over SSH to a runner process (`rk agent`) on each machine, as `rucc-postgres` does.

Rough capacity:
- a 7.2 `defconfig` build is about 3 minutes on gpc and 10 on server3; `allmodconfig` is about 35 minutes on gpc;
- a Releases sweep of 114 trees × 2 rows × (build + boot + KUnit) is about 114 × 2 × 10 minutes, or about 38 hours of gpc time. That is a weekly job, on the edge of fitting, and it runs incrementally (only trees whose inputs changed, meaning the rucc hash) with the rest of the week's rucc commits batched;
- the monthly sets are about 90 + 5 trees.

## 13.7 Storage

Per run we keep:
- the manifest, the `compile.jsonl` summary (not every line; full logs for 7 days), results, dmesg, and diffs;
- the kernel images for failures only, pruned after 30 days.

Baselines are kept forever, and they are small (under 50 MB per row and version). The tree cache is about 16 GB (04.5) and the containers about 30 GB. The laptop this plan was written on has about 4 GB free, so none of this runs there. gpc and server3 need at least 500 GB each for build scratch space. That is open question 7.

## 13.8 Reports

`reports/nightly.md` follows `rucc-postgres`'s layout:
1. **Headline.** Per row: set members green, red and not run.
2. **Regressions since last night**, each with its bisected rucc commit, found by `git bisect` over rucc commits with `rk build`/`rk test` as the test.
3. **Differentials.** Config, flags, sections, symvers, vec-audit, objtool: counts with links to details.
4. **Tests.** Per suite, pass counts against the reference's G.
5. **Performance.** Static measures nightly; runtime benchmarks when server3 ran them.
6. **Compile time and memory.** Ratio against GCC, the slowest 10 units.
7. **Demands.** Missing features by blocked unit count.
8. **Open findings** with issue links.
