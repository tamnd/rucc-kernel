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

The reference compilers live in era containers built from `provision/eras` and pinned in `toolchains.toml`. `rk personas check` runs each one and fails when its GCC or binutils is not the version `personas.toml` names for the era, or when GCC finds plugin headers, which would turn on `GCC_PLUGINS` for the reference and never for rucc.

`rk build 7.2.8 --row X64 --config defconfig --fragment test --cc gcc-14` configures and builds a kernel through `rk-cc`, with the test fragment from `configs/` merged in. Pass `--cc rucc` for rucc and `--keep-going` to see every failing unit, not just the first. The build directory ends up with `build.json`, `summary.md` and `compile.jsonl`, which has one line per compiler call. For a probe that line also keeps what the probe read on standard input and printed on standard output, so tools like the `kernel-probes` corpus in rucc-compat can ask it again, and `build.json` names the source tree and the era's `__GNUC__` version and `-std=` so they can give rucc the same persona.

`rk boot --build DIR` boots a build under QEMU with `rk-init/init.sh` as PID 1 and a static busybox, and reads the smoke checks off the serial console. `rk baseline` builds and boots with the reference three times and writes the result under `results/baseline`.

`rk test --reference DIR --other DIR --kinds boot,smoke,kunit` is the graded run. It boots the reference kernel and the rucc kernel once per suite, with `rk.suite=` on the command line, and compares them unit by unit: `boot`, each smoke check, each KUnit result read from the TAP on the console, and each test module loading. The kunit boot carries the build's modules, so build with `--targets "bzImage modules"` when the test fragment makes KUnit tests modules. The units the reference passes in every run (`--runs`) are graded and the rucc kernel must pass all of them. The splats in both consoles are normalized, and one that only the rucc kernel prints fails the run too. Given a rucc-built busybox with `--rucc-busybox`, every failure is attributed to the kernel, the userland or the two together by running the other two cells of the 2x2. kselftest and LTP are refused until K5. The run directory gets `test.json`, `summary.md` and the console of every boot.

`rk mixed --reference DIR --other DIR --unit UNIT` finds what makes one unit that `rk test` failed go wrong. Every object the two builds have with different bytes is a candidate. A trial copies the reference build, puts some rucc objects in place of the reference's, relinks with the reference's own make command so that nothing is compiled again, and boots the suite that reports the unit. Delta debugging finds a smallest set of rucc objects that still fails. When one object is left and the other build is rucc's, the object is compiled again with `-fpass-fuel-global=N` and the fuel is bisected to the first transformation that breaks the unit, and the `-fdump-ir=all` dumps on each side of it name the pass and the functions it changed. A trial that does not link is a finding of its own and ends the search. The run directory gets `mixed.json`, `summary.md`, the console of every boot and the dumps.

`rk cross-modules --reference DIR --other DIR` loads the rucc modules into the reference kernel and the reference modules into the rucc kernel. It first checks every module against the other kernel without booting: the relocation types, the whole `vermagic`, symbols the kernel does not export and CRCs that differ from its `Module.symvers`. Then it runs the kunit suite three times per run: the reference kernel with its own modules, which finds the units that pass in QEMU without hardware, and then each kernel with the other build's modules. Both crossed boots must pass every unit the first one passed, and a splat only a crossed boot prints fails the run. Both builds need `--targets "bzImage modules"`. The run directory gets `cross-modules.json`, `summary.md` and the console of every boot.

`rk config-diff`, `rk probes`, `rk flags-diff`, `rk demands` and `rk asm-inventory` read build directories and write markdown tables: the `.config` differences, the compiler probes the two compilers answered differently, the flags each unit was compiled with on one side only (read from the `.cmd` files kbuild writes), the failed units by error, and the instructions the kernel writes itself. `rk config-diff --why` also names the Kconfig expressions behind each difference and the probe that decided them, and needs the kernel tree, which it finds through `build.json` or `--source`. `rk syntax --build DIR --cc rucc` replays every unit of a reference build through rucc's front end with the reference's own command lines, `-fsyntax-only` for C and `-E` for assembly, and writes the failing units grouped by their first error. Units listed with an issue in `syntax-known.toml` are known failures, so the command fails only on new ones.

`rk sections-diff`, `rk symvers-diff`, `rk vec-audit`, `rk modules-audit` and `rk objtool-report` look at what the compilers produced. `rk sections-diff` compares the section names of every object two builds share, the size and relocation count of each kernel table (exports, jump labels, alternatives, exception fixups, initcalls and the rest), the call-site lists per function, and the `.modinfo` and `__ksymtab_strings` strings. `rk symvers-diff` compares `Module.symvers`, CRCs and namespaces included, and the global symbols of `System.map`. `rk vec-audit` decodes the x86-64 code of every unit built with `-mno-sse` or `-mgeneral-regs-only` and lists the vector and x87 instructions it finds, only those above the reference's count when given `--reference`. `rk modules-audit` fails on any module relocation the loader rejects, a wrong `vermagic` or an imported CRC that does not match `Module.symvers`. `rk objtool-report` buckets the objtool warnings of two build logs by message and function shape. `rk frames` compares the stack frame of every function in two builds made with `rk build --stack-usage`, which passes `KCFLAGS=-fstack-usage` so that each compiler writes a `.su` file next to every object. It fails on a function over `CONFIG_FRAME_WARN` that the reference keeps under it, and on a run time stack high water mark more than 10% above the reference's when both build directories have a `boot.log` from a kernel with `CONFIG_DEBUG_STACK_USAGE`. `rk sections-diff`, `rk vec-audit` and `rk frames` take `--save FILE` to keep what they read as JSON, and accept such a file in place of a build directory, which is how CI compares builds without moving their objects.

The nightly workflow builds every pin in the current set on X64 with gcc-14 and with rucc, from `tinyconfig` and the test fragment, and runs `rk test` on boot, smoke and KUnit. When the rucc kernel fails a unit, `rk mixed` runs on the first one in the same job. The run's summary has a line per version, green or red, and what `rk mixed` found. A manual run takes a branch or commit of tamnd/rucc and another configuration target.

## House style

Prose in this repository is plain English with one paragraph per line, no em or en dashes and no horizontal rules. `scripts/style.sh` checks it on every pull request.

## License

Apache-2.0. The kernel trees the harness fetches are under their own licenses and are never stored here.
