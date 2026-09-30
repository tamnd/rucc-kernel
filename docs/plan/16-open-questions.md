# Open questions

These are ranked by how much of the plan changes with the answer. Each one says:
- what we believe now;
- what goes wrong if the belief is wrong;
- how to find out;
- the latest milestone by which it must be settled.

A question past its deadline blocks that milestone.

## 1. What the persona banner says, and whether the GNU assembler banner is honest

**Belief.** Under a persona, a first line of `gcc (rucc 0.17.0, GNU C persona 14.2.0) 14.2.0` (04.2.1) satisfies:
- 4.18 to 5.11's `grep -q gcc`;
- 5.12's `cc-version.sh`, which reads macros, not the banner;
- `CONFIG_CC_VERSION_TEXT`.

It is honest, because it names rucc and says "persona". `-Wa,--version` printing `GNU assembler (rucc 0.17.0 integrated) 2.44` is the only path `as-version.sh` accepts for a non-Clang compiler.

**Risk.**
- **Tools other than kbuild parse `gcc --version`.** Examples are distribution packaging scripts, `dkms` and out-of-tree module builders. They may then take GCC-specific paths, such as passing plugin flags.
- **"GNU assembler" in a non-GNU program's output is a claim about identity that some would call misleading.**
- **Upstream kernel people may object** if the harness ever reports results publicly under that banner.

The alternative is an upstream kernel change: teach `cc-version.sh` and `as-version.sh` a third compiler family (as ICC once was). That is a patch to the kernel, which is fine upstream and never in our harness. It also takes years to reach LTS trees, and never reaches old ones.

**How to find out.**
- Grep kbuild in every era for every use of `--version`, `-v`, `-dumpversion` and `-Wa,--version`, and list what each expects (`rk probes` already collects the invocations).
- Grep `dkms`, Debian's `linux-kbuild` scripts and Fedora's `kernel.spec` for banner parsing.
- Ask on `linux-kbuild@vger.kernel.org` whether a `CC_IS_RUCC`-style family would be welcome, as `CC_IS_CLANG` was. The answer decides whether the banner is permanent or a bridge for old trees only.

**Deadline.** K1 for the banner, since K1 depends on it. The mailing list question can run in parallel with no deadline.

## 2. Does objtool accept rucc's code shapes, and what if it doesn't

**Belief.** Most of objtool's complaints will be about a few things:
- jump table layout;
- noreturn agreement;
- frame setup order;
- the end of functions after `__builtin_unreachable`.

rucc can produce GCC's shapes for all of them (09.1), and three weeks of K3 covers it.

**Risk.** objtool's control flow and stack state analysis encodes GCC's and Clang's habits in places that are not documented: DRAP realignment forms, how `alternatives` interact with stack state, and `.cold` handling. A rucc shape that is correct and unrecognized could need objtool changes. Our rule forbids patching the harness's kernel, so the fix would have to land upstream and be backported, or rucc must imitate GCC exactly, which may cost code quality. If this goes badly, K3 grows from 6 weeks to 10 or more, and old kernels (K9, K10) each have their own objtool version with its own habits.

**How to find out.** At K2, before `vmlinux` links, run the pinned objtool over every rucc object of 7.2 `defconfig` and bucket the warnings (`rk objtool-report`). Do the same with the 4.19 and 5.10 objtool. The bucket count and the shapes behind them answer this within a week of K2's first complete object set.

**Deadline.** K2's midpoint. That early run is part of K2's work.

## 3. GNU ld or ld.lld

**Belief.** GNU ld from each era's binutils is the graded linker, because it is what the reference used and what every era supports. `ld.lld` is an ungraded second column on Current (09.2).

**Risk.**
- Some Current configurations may produce different results with the two linkers even for GCC (orphan section handling, relaxation on riscv).
- If users of rucc are mostly LLVM-toolchain users, the useful claim is "rucc plus lld".

The choice does not affect rucc's work much. Both consume the same ELF.

**How to find out.** Build the reference with `LD=ld.lld` on 7.2 `defconfig` for X64 and A64, and compare the results with GNU ld's. If both are green with GCC objects, grading rucc with both costs only machine time.

**Deadline.** K5.

## 4. What the museum really includes: a.out, and 0.x

**Belief.**
- 1.2.13 can be built as ELF with the right toolchain.
- 1.0 needs a.out objects linked by an a.out `ld`, so rucc needs an a.out writer.
- 0.01 to 0.99 need a Minix-hosted or GCC 1.x-era toolchain, and a boot environment that QEMU can still provide (floppy, Minix filesystem).

The museum set (04.1.4) includes 1.0 and excludes 0.x.

**Risk.**
- **The a.out writer is useless for anything else.** It is 2 weeks of work for one tree.
- **1.0 may not boot under QEMU's current machine models at all.** Old kernels probe hardware QEMU no longer emulates by default: specific IDE, floppy and PIT behaviour.
- **It may not even build with a GCC 2.x reference** we can reconstruct. If the reference cannot be made to work, 1.0 cannot be graded (02.3 needs G), and the effort buys nothing.

**How to find out.** At K0, build 1.0, 1.2.13 and 2.0.40 with the E0 and E1 reference toolchains, and boot them in QEMU with `-machine isapc` and a floppy image. Whatever works with the reference defines the museum. 0.01 gets the same experiment, time-boxed to 2 days.

**Deadline.** K0 for the experiment, K10's start for the decision (K11's scope).

## 5. The size of the assembler work

**Belief.** The macro processor, expression relaxation and per-object stream are 5 to 6 weeks (15, K2). The instruction tables come from the `rk asm-inventory` census plus the existing x86-64 encoder and are mostly mechanical.

**Risk.**
- **The per-object stream is an architectural change.** It moves inline asm from "lowered to machine IR" to "text fragments in one assembler stream". It touches the register allocator's view of asm blocks, branch relaxation and scheduling. It could be much larger than estimated, and it is on the critical path of everything.
- **The encoder may disagree with gas on instruction length or form** in ways that only show in `alternatives` padding.

**How to find out.** At K0, spike the design: take `arch/x86/include/asm/alternative.h`'s `ALTERNATIVE` plus one `_ASM_EXTABLE` user, and prototype the stream for that one object, in a branch of rucc. Measure against `kernel-asm` for that object. The spike's cost and what it touches re-estimate K2.

**Deadline.** K0's end.

## 6. Hardware for AArch64 performance

**Belief.** 02.5's runtime performance targets are graded on X64 only, because we have no AArch64 hardware with KVM. AArch64 TCG numbers are a sanity check.

**Risk.** The AArch64 back end is younger and less tuned than x86-64. A claim of "very high performance on all versions" that is only measured on x86-64 is half a claim.

**Options.**
- A GitHub `ubuntu-24.04-arm` runner, if it exposes `/dev/kvm` (question 8). But runners are noisy neighbours, which is poor for a 5% gate.
- A rented Ampere Altra or Graviton bare-metal instance for monthly runs. That is about 1 to 2 USD an hour, and a full 12.2 run is about 3 hours.
- An Apple Silicon Mac with a Linux VM under Hypervisor.framework. This laptop is one, but it is not always on, and nested virtualization is not needed since the VM is the guest.

**How to find out.** Run the 12.2 suite twice on each candidate with the GCC kernel only, and measure the run-to-run variation. The machine with a coefficient of variation under 1% on B1 to B9 wins.

**Deadline.** K8.

## 7. Disk, CI time and machines

**Belief.**
- **Storage:** about 16 GB of trees, 30 GB of containers, 500 GB of scratch per build machine (13.7).
- **Time:** the weekly Releases sweep is about 38 hours of gpc time (13.6).
- **gpc** is available as a worker.

**Risk.** gpc is not in `test-machines.md`, and its availability is assumed, not agreed. This laptop has 4 GB free and cannot run any of it. If the sweep does not fit a week, Releases must become incremental-only or be split across machines, and "every version, every week" becomes "every version, every month".

**How to find out.** Confirm gpc's disk and availability. Time one `defconfig` build and boot of 2.6.32, 3.10, 4.19 and 7.2 on gpc at K0, and compute the sweep from measurements.

**Deadline.** K0.

## 8. KVM on GitHub arm runners

**Belief.** Unknown. GitHub's larger x86 runners have exposed `/dev/kvm` since 2024. For `ubuntu-24.04-arm` it is not documented.

**Risk.** Without KVM, A64 boots run under TCG: 10 to 30 times slower. A nightly A64 kselftest and LTP run then does not fit on server1 and server2, and A64 grading has to thin out.

**How to find out.** One workflow run that checks `/dev/kvm` and boots a 7.2 arm64 `defconfig` with `-accel kvm`.

**Deadline.** K0 (an hour's work). The answer shapes K6's machine plan.

## 9. Distro configs that need what rucc cannot do

**Belief.** Debian 13's amd64 and arm64 configs enable features rucc will not have by K5:
- UBSAN bounds on some versions (check);
- `DEBUG_INFO_BTF` (fine);
- `GCC_PLUGINS`, which we exclude by the container (05.4);
- the stack protector (fine after K2).

**Risk.**
- If Debian's config enables a sanitizer, the "distro" configuration cannot have an identical `.config`, and K5's criterion cannot be met without a sanitizer implementation (XL).
- Fedora enables more hardening than Debian.

**How to find out.** Run `rk probes` and `rk config-diff` on Debian 13 and Fedora 44 configs at K1, and list every difference that no planned rucc work removes.

**Options.**
- Choose the distro config that works.
- Implement `-fsanitize=bounds` (M; it is the simplest UBSAN mode, a check and a call).
- Accept that the distro row is compile-only for that version.

**Deadline.** K1 for the census, K5 for the decision.

## 10. Which 3.x and 4.x stable branches can be built by a later GCC

**Belief.** Every tree is graded with its own era's reference (04.2), so the question of later GCC support in old trees only matters for choosing era boundaries. For example, 3.x stable branches backported `compiler-gcc5.h` and later `compiler-gcc6.h`, so the last points of 3.16 and 3.18 may build with newer GCC than their releases.

**Risk.** An era boundary set by the release may be wrong for the last point release of a long branch, and a last-point failure would be blamed on rucc when it is the era choice.

**How to find out.** `rk personas check` over the Last points set builds each with the era reference and the next era's. Split eras where they disagree.

**Deadline.** K10.

## 11. CIP 4.19 end of life and whether 4.4-cip stays

**Belief.** CIP maintains 4.4 into January 2027 and 4.19 to around 2029. Both are in Current.

**Risk.** Only a small risk. If CIP extends 4.4, it stays in Current longer, and its era (E7) has the most fragile persona paths.

**How to find out.** CIP's wiki and release announcements. `rk sets` reads the EOL dates from a pinned copy.

**Deadline.** None. `rk sets` tracks it.

## 12. Rust becoming necessary

**Belief.** `CONFIG_RUST=n` builds every configuration we grade. No subsystem in 7.2 that `defconfig` or the Debian config enables requires Rust.

**Risk.** Rust drivers become the only driver for some hardware (Apple GPU, NVMe variants, `nova` for NVIDIA) or for a core facility (the Rust binder, `rnull`, QR code panic). A future distro config may enable Rust, and `CONFIG_RUST=y` changes `.config` and adds `MODVERSIONS` through `gendwarfksyms`. The C part is then compiled by rucc and the Rust part by `rustc` with `bindgen` reading headers via libclang, a GCC-plus-Clang mix that the kernel supports but that complicates our "same kernel" rule.

**How to find out.** Check each new release's `defconfig` and Debian and Fedora configs for `RUST=y`. `rk sets` flags it.

**Deadline.** None. Revisit when a graded config enables Rust. At that point the claim becomes "the C parts are compiled by rucc", with `rustc` and `bindgen` pinned as reference tools alongside `ld`.

## 13. Whether the parent spec accepts the redefinitions

**Belief.** rucc's spec will accept three changes:
- i686 as M10's fourth target;
- M11's levels mapped to K3, K5 and K8;
- `-O2` and `-Os` as the kernel's graded levels.

**Risk.** If M10's fourth target must be something else (riscv64 is the other obvious candidate), K4 stays on the critical path anyway, because the x86-64 kernel needs it. There would then be five targets.

**How to find out.** Propose the edits at K0.

**Deadline.** K0.
