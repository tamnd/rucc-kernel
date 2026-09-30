# rucc-kernel

A harness that builds pinned, unmodified Linux kernels with [rucc](https://github.com/tamnd/rucc) and with a reference GCC, boots both under QEMU, runs the kernel's own tests against them, and records what happened in files that outlive any one report.

The goal is simple to state and slow to reach: every Linux release, from the museum trees to the newest stable, built by rucc, booted, passing the tests the GCC build passes, and running about as fast. The plan that gets there is in [docs/plan](docs/plan), split into thirteen milestones, K0 to K12. Each milestone has a tracking issue and a GitHub milestone of the same name, and its checklist is ticked as the pull requests land.

## The rules

**No compiler code.** Nothing here compiles C. Every compile goes to a real compiler, rucc or the reference, through the `rk-cc` shim, which records the call and changes nothing. A fix to rucc goes to [tamnd/rucc](https://github.com/tamnd/rucc).

**No kernel patches and no copied kernel files.** Trees are fetched from kernel.org by hash. The harness never writes into a tree, apart from kbuild's own output under `O=`.

**No reductions.** A reduced test case goes to [rucc-corpus](https://github.com/tamnd/rucc-corpus), or to [rucc-compat](https://github.com/tamnd/rucc-compat) when it compares two compilers. This repository links to it by issue number.

**No hidden configuration.** Everything that changes a graded build is in a committed file: pins, personas, rows, configs and the list of explained differences.

## Rows

A row is an architecture with a QEMU machine and a reference toolchain. X64 is x86-64 and comes first. A64 is arm64. X32 is i686, which the x86-64 kernel needs anyway for its real mode setup code and its 32-bit vDSO. R64 is riscv64 and waits for rucc to have a RISC-V back end.

## Milestones

| | Scope |
|---|---|
| K0 | the instrument: pins, eras, the shim, baselines, probes and the demand census |
| K1 | identity and front end: kbuild accepts rucc and `.config` comes out identical |
| K2 | every x86-64 `defconfig` unit compiles and `vmlinux` links |
| K3 | first graded boot of x86-64 `tinyconfig` with KUnit, objtool clean |
| K4 | i686, and x86-64 `defconfig` with nothing delegated |
| K5 | x86-64 distro config, `allmodconfig`, kselftest and LTP |
| K6 | arm64 at K5's level |
| K7 | riscv64 `defconfig` boots |
| K8 | the performance gate |
| K9 | every longterm and CIP head |
| K10 | every release from 2.6.12 |
| K11 | the museum, 1.0 to 2.4 on i386 |
| K12 | steady state |

## Building

`cargo build --release` builds `rk`. The rust toolchain is pinned in `rust-toolchain.toml`. The kernel builds themselves need Linux, GNU make, flex, bison, bc and the usual kernel build dependencies, and QEMU for the boots.

## Using rk

`rk fetch 7.2.8` downloads a pinned tree into `~/.cache/rk` (or `RK_CACHE`), checks its SHA-256 and unpacks it. `rk sets` compares `pins.toml` with what kernel.org lists today and rewrites it with `--write`. `rk personas` prints the era, the GCC version rucc claims and the reference toolchain for every pin.

`rk build 7.2.8 --row X64 --config defconfig --fragment test --cc gcc-14` configures and builds a kernel through `rk-cc`, with the test fragment from `configs/` merged in. Pass `--cc rucc` for rucc and `--keep-going` to see every failing unit, not just the first. The build directory ends up with `build.json`, `summary.md` and `compile.jsonl`, which has one line per compiler call.

`rk boot --build DIR` boots a build under QEMU with `rk-init/init.sh` as PID 1 and a static busybox, and reads the smoke checks off the serial console. `rk baseline` builds and boots with the reference three times and writes the result under `results/baseline`.

`rk config-diff`, `rk probes`, `rk demands` and `rk asm-inventory` read build directories and write markdown tables: the `.config` differences, the compiler probes the two compilers answered differently, the failed units by error, and the instructions the kernel writes itself.

## House style

Prose in this repository is plain English with one paragraph per line, no em or en dashes and no horizontal rules. `scripts/style.sh` checks it on every pull request.

## License

Apache-2.0. The kernel trees the harness fetches are under their own licenses and are never stored here.
