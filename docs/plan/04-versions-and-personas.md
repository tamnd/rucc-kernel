# Versions, personas and reference toolchains

"All versions of Linux" means about 950 tags and several thousand point releases, built by a dozen generations of GCC. This document fixes which trees are graded, which GCC rucc pretends to be for each, and which real GCC builds the reference kernel it is compared against.

## 4.1 The four sets

The sets are defined by rules, not by lists. `rk sets` expands each rule against `kernel.org` and the full history mirror into `pins.toml`, and the expanded list is committed, so a graded run always names exact trees.

### 4.1.1 Current

Nightly. Rows X64 and A64 get the full grade, X32 builds and boots `defconfig`, and R64 joins once K7 lands.

| Member | Today (30 Sep 2026) | Rule |
|---|---|---|
| mainline | `v7.3-rc5`, and `master` at the nightly's start | the newest tag on `torvalds/linux`, plus the branch head as an ungraded canary |
| stable | 7.2.8 | the newest point of the newest stable branch |
| longterm | 6.18.54, 6.12.111, 6.6.157, 6.1.188, 5.15.221, 5.10.270 | the head of every branch `kernel.org/releases.json` marks `longterm` |
| CIP SLTS | 4.4.y-cip, 4.19.y-cip, 5.10.y-cip, 6.1.y-cip, 6.12.y-cip | the head of every CIP branch not past its EOL date |

The 5.10 and 5.15 longterm branches reach end of life in December 2026. They stay in Current as CIP heads (5.10) or drop to Last points (5.15) on that date. Nothing is removed silently: `rk sets` writes a diff of the expansion into the nightly report.

### 4.1.2 Releases

Every non-rc tag from `v2.6.12`, the first in git, to the newest release. There are 114 today. New tags are added the day they appear and graded that night. The whole set is swept weekly.

| Row | Tags |
|---|---|
| X64 | all 114. `ARCH=x86_64` before 2.6.24, `ARCH=x86` after |
| X32 | all 114. `ARCH=i386` before 2.6.24 |
| A64 | from `v3.7`, the first with `arch/arm64` |
| R64 | from `v4.15` (first boot on `virt`), once K7 lands |

### 4.1.3 Last points

The last point release of every stable and longterm branch there has ever been, on X64 and X32, monthly. Before 2.6.12 the branches are 2.4 and 2.2, covered by Museum, and 2.6.x.y tags start at 2.6.11.1. That gives about 90 trees today. Examples: 2.6.16.62, 2.6.27.62, 2.6.32.71, 3.2.102, 3.4.113, 3.10.108, 3.16.85, 3.18.140, 4.4.302, 4.9.337, 4.14.336, 4.19.325, 5.4.302.

These matter because the long branches picked up compiler work the release they came from never saw: gcc 5, 6 and 7 support headers, `-fno-PIE` handling, retpoline, objtool fixes. The last point of a branch is also what the world actually ran.

### 4.1.4 Museum

X32 only, monthly, built and booted to `rk-init` (document 11.6).

| Tree | Why this one |
|---|---|
| 1.0 | the first release, March 1994 |
| 1.2.13 | the end of 1.x, and the first with ELF optional |
| 2.0.40 | the end of 2.0, the first SMP kernel |
| 2.2.26 | the end of 2.2 |
| 2.4.37.11 | the end of 2.4, and the last before git |

0.01 to 0.99 are open question 4. They are listed but not graded.

## 4.2 Eras

An era is a range of versions that share a reference toolchain, a persona, a dialect default and a host container. The boundaries are where one of those four must change.

| Era | Versions | Reference GCC and binutils | Container | rucc persona (`-fgnuc-version=`) | Dialect default | Identity check the kernel runs |
|---|---|---|---|---|---|---|
| E0 museum | 1.0 to 1.2.13 | GCC 2.5.8 and 2.7.2.3 (1.2), binutils 2.5 era with a.out | a built from source toolchain on `debian/etch` i386 | 2.7.2 | gnu89, `-traditional` in `.S` | none |
| E1 | 2.0.40, 2.2.26 | GCC 2.7.2.3, binutils 2.9.1 | `debian/etch` i386, built from source | 2.7.2 | gnu89 | none |
| E2 | 2.4.37.11 | GCC 2.95.3 (2.4 prefers it), binutils 2.12 | `debian/etch` i386, built from source | 2.95.3 | gnu89 | none |
| E3 | 2.6.12 to 2.6.15 | GCC 3.4.6, binutils 2.17 | `debian/etch` | 3.4.6 | gnu89 | `compiler-gcc{2,3,4}.h` by `__GNUC__` |
| E4 | 2.6.16 to 2.6.39 | GCC 4.1.2 up to 2.6.25, then 4.3.2 | `debian/etch`, then `debian/lenny` | 4.1.2, then 4.3.2 | gnu89 | `compiler-gcc{3,4}.h` |
| E5 | 3.0 to 3.17 | GCC 4.7.2 | `debian/wheezy` | 4.7.2 | gnu89 | `compiler-gcc{3,4}.h` |
| E6 | 3.18 to 4.1 | GCC 4.9.2 | `debian/jessie` | 4.9.2 | `-std=gnu89` passed | `compiler-gcc{3,4,5}.h`, so a GCC 6 persona fails |
| E7 | 4.2 to 4.17 | GCC 6.3 | `debian/stretch` | 6.3.0 | `-std=gnu89` | unified `compiler-gcc.h`, `cc-name` by `-v`, `gcc-version.sh` |
| E8 | 4.18 to 5.11 | GCC 8.3 up to 5.4, then 10.2 | `debian/buster`, then `debian/bullseye` | 8.3.0, then 10.2.1 | `-std=gnu89` | Kconfig: first line of `--version` contains `gcc` |
| E9 | 5.12 to 5.17 | GCC 10.2 | `debian/bullseye` | 10.2.1 | `-std=gnu89` | `cc-version.sh` via `-E` macros; `as-version.sh`; `min-tool-version.sh` from 5.13 |
| E10 | 5.18 to 6.14 | GCC 12.2 | `debian/bookworm` | 12.2.0 | `-std=gnu11` | the same |
| E11 | 6.15 to current | GCC 14.2 and binutils 2.44 from `debian/trixie`, plus GCC 16.2 as a second reference on Current | `debian/trixie` | 14.2.0 | `-std=gnu11` | the same, and minimum 8.1 |

Rules behind the table:

1. **The reference is the GCC that a distribution shipped when the version was current.** Where there are two candidates, pick the one the kernel's own documentation or `Documentation/Changes` recommended. It is the toolchain the version was tested with, which minimizes the reference's own warnings and failures. The newest GCC that builds a version is a different, stricter test. It is run as a probe, not graded, because an old kernel failing under GCC 16 is not rucc's problem.
2. **The persona equals the reference's version**, so that every version test in the tree takes the same branch for both builds. That is the only way 2.2's `.config` equality can hold. The exception is Current, where the persona is the primary reference (14.2) and the GCC 16.2 build is compared on everything but `.config`.
3. **The persona's behaviour follows the persona's version** in the few places GCC's changed and the kernel noticed:
   - the default `-std`, gnu89 up to 4.9 and gnu11 or later after;
   - `-fno-common` default from GCC 10;
   - which `-W` flags `cc-option` sees as known.

   rucc does not emulate old GCC bugs. Where an old kernel depended on one (for example, 2.4's reliance on GCC 2.95 accepting some constructs as lvalues), rucc accepts the construct under the old persona, as a dialect feature. It never reproduces a miscompilation.
4. **An era boundary moves only with evidence.** If a version inside an era fails with the era's reference but builds with a neighbour's, split the era and record the reason in `personas.toml`.

### 4.2.1 The version-string problem under an honest name

The kernel reads the compiler's identity in four ways, and a persona must satisfy each in its era:

| Mechanism | Eras | What rucc must answer |
|---|---|---|
| `__GNUC__`, `__GNUC_MINOR__`, `__GNUC_PATCHLEVEL__` in the preprocessor | all | the persona's numbers. Already works |
| the first line of `$(CC) --version` | E8 (grep `gcc`), and every era for `CONFIG_CC_VERSION_TEXT` | a line that contains `gcc`, the persona's version and `rucc` |
| `$(CC) -v` containing `clang version` | E7 (`cc-name`) | it must not |
| `-dumpversion`, `-dumpfullversion` | E7, E8 scripts, GCC plugin checks | the persona's version. Already works for `-dumpversion` |

The proposed banner under a persona is:

```
gcc (rucc 0.17.0, GNU C persona 14.2.0) 14.2.0
```

Without a persona it stays `rucc 0.17.0`. This banner is honest, in that it names rucc and the word "persona", and it matches `grep -q gcc`. The GCC version is its last word, which is the field `scripts/cc-version.sh` and `CONFIG_CC_VERSION_TEXT` consumers read. Whether shipping that banner is acceptable is open question 1. CCC shipped `ccc (Claude's C Compiler, GCC-compatible) 14.2.0` and hit the same grep.

### 4.2.2 The assembler persona

From 5.12, `scripts/as-version.sh` runs `$(CC) -Wa,--version -c -x assembler /dev/null` and wants `GNU assembler` followed by a version, or it takes the LLVM integrated assembler path when it sees `clang` in `$(CC)`. rucc has one assembler, inside itself. Two options:

1. Print `GNU assembler (rucc 0.17.0 integrated) 2.44`, with the binutils version taken from a second persona value, `-fgnu-as-version=2.44`. The kernel then gates `AS_HAS_*` probes on the version and runs the assembler probes (`as-instr`) against rucc's assembler, which is what actually assembles.
2. Report as LLVM's integrated assembler. That requires `CC_IS_CLANG`, which contradicts the GCC persona.

Option 1 is chosen. `CONFIG_AS_VERSION` is then listed in `config-divergences.toml` only if the reference's binutils differs from the value we print, and we print the reference's. The `as-instr` probes are the real test. `AS_AVX512`, `AS_SHA256_NI`, `AS_TPAUSE`, `AS_GFNI`, `AS_VAES`, `AS_VPCLMULQDQ`, `AS_WRUSS` and the arm64 `AS_HAS_*` family each gate a crypto or feature file. A rucc assembler that says no where gas says yes changes the `.config`, and 2.2 catches it.

## 4.3 `personas.toml`

One entry per era, and nothing is inferred at run time.

```toml
[[era]]
id = "E11"
versions = ">=6.15"
reference = { gcc = "14.2.0", binutils = "2.44", container = "rk-era-trixie" }
second-reference = { gcc = "16.2.0", binutils = "2.45", container = "rk-era-forky" }
persona = { gnuc = "14.2.0", gnu-as = "2.44" }
cc = "rucc -fgnuc-version=14.2.0 -fgnu-as-version=2.44"
host = "rk-era-trixie"
notes = "min gcc 8.1 from 6.16; gnu11 from Makefile"

[[era]]
id = "E6"
versions = ">=3.18, <4.2"
reference = { gcc = "4.9.2", binutils = "2.25", container = "rk-era-jessie" }
persona = { gnuc = "4.9.2", gnu-as = "2.25" }
cc = "rucc -fgnuc-version=4.9.2 -fgnu-as-version=2.25"
host = "rk-era-jessie"
notes = "compiler-gcc5.h is the newest header; a gnuc>=6 persona fails by design"
```

`rk personas check` builds every era's first and last version with the reference and fails if the rule in 4.2 no longer holds. For example, if a Debian archive rebuild changed the GCC patch level.

## 4.4 Era containers

Every era has a container image, built by `rucc-kernel/provision/eras/<id>/`:

| Image | Base | Contents |
|---|---|---|
| `rk-era-etch` | `debian/eol:etch` from the Debian snapshot archive, i386 and amd64 | make 3.81, perl 5.8, GCC 4.1.2, GCC 3.4.6 and binutils 2.17, plus from-source GCC 2.7.2.3 and 2.95.3 and binutils 2.9.1 and 2.12 under `/opt` |
| `rk-era-lenny`, `-wheezy`, `-jessie`, `-stretch`, `-buster`, `-bullseye`, `-bookworm`, `-trixie` | `debian/eol:<name>` or the official image | the distribution's GCC, binutils, make, perl, bc, bison, flex, libelf, openssl, the cross binutils for arm64 from wheezy on |
| `rk-era-forky` | `debian:testing` | GCC 16, the second reference |
| `rk-rucc` | any of the above plus a mounted `rucc` binary | rucc is statically linked (musl) so it runs in all of them |

The era image supplies everything but the compiler under test: `HOSTCC`, `make`, `perl`, `bc`, `bison`, `flex`, `depmod`. That is how 2.1's "no host tool the era did not have" holds. Two old-tree failures show why:
- `kernel/timeconst.pl` in 2.6.33 to 3.x uses `defined(@array)`, which perl 5.22 made fatal;
- `scripts/dtc` before 5.x declares `yylloc` twice, which GCC 10's `-fno-common` default rejects.

Both disappear in the era's own container, and neither is rucc's business.

rucc runs inside the container as a static binary. It must be built with `--target x86_64-unknown-linux-musl`, and `rucc-kernel` checks that the binary has no dynamic section.

Cost: about 30 GB of images. They are built once, pushed to the GitHub container registry under `tamnd/rucc-kernel`, and pinned by digest in `toolchains.toml`.

## 4.5 Pinning trees

`pins.toml` records each tree as:

```toml
[[tree]]
version = "7.2.8"
source = "https://cdn.kernel.org/pub/linux/kernel/v7.x/linux-7.2.8.tar.xz"
sha256 = "..."
era = "E11"
sets = ["current", "releases"]
```

- 2.6.12 and later come from `kernel.org` tarballs, which have been stable since 2.6.
- The Museum trees come from the `kernel.org` historic directories, with SHA-256 recorded at first fetch and compared with the full history mirror (`mpe/linux-fullhistory`) where it has them.
- Tarballs live in a content-addressed cache on each machine. At about 140 MB compressed for recent trees, 114 releases plus 90 last points is roughly 16 GB, most of it pre-4.0 and much smaller.
- The disk budget is open question 7.

A build unpacks into a scratch directory and deletes it afterwards, unless `--keep` is passed. That leaves one unpacked tree per concurrent job: about 1.5 GB for a recent tree, plus 1 to 3 GB for the output of `defconfig` and 30 GB for `allmodconfig`.
