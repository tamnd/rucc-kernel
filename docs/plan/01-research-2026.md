# Research, September 2026

These are the facts every other document relies on. Items checked against the kernel tree at the named tag are marked **[tree]**. They were read from `raw.githubusercontent.com/torvalds/linux/<tag>/...` or from `git.kernel.org` plain views. Items taken from secondary sources carry a link. Items that are uncertain say so, and K0's first job is to settle them with `git log -S` on a local clone.

## 1.1 Where the kernel is today

Taken from `kernel.org/finger_banner` and `releases.json`, fetched 30 September 2026.

| Channel | Version | Date |
|---|---|---|
| mainline | 7.3-rc5 | 2026-09-27 |
| stable | 7.2.8 | 2026-09-25 |
| stable, end of life | 7.1.13 | 2026-09-02 |
| longterm | 6.18.54 | 2026-09-25 |
| longterm | 6.12.111 | 2026-09-21 |
| longterm | 6.6.157 | 2026-09-14 |
| longterm | 6.1.188 | 2026-09-14 |
| longterm | 5.15.221 | 2026-09-14 |
| longterm | 5.10.270 | 2026-09-14 |

Release dates of the recent mainline versions:

| Version | Date | Note | Source |
|---|---|---|---|
| 6.19 | 2026-02-08 | the last 6.x | |
| 7.0 | 2026-04-12 | Rust lost its experimental label | [Phoronix](https://www.phoronix.com/news/Linux-7.0-Released) |
| 7.1 | 2026-06-14 | | |
| 7.2 | 2026-08-16 | | [Phoronix](https://www.phoronix.com/news/Linux-7.2-Released) |
| 7.3 | expected late October 2026 | | |

Longterm end of life, per `kernel.org/releases.html`:

| Branch | End of life |
|---|---|
| 5.10, 5.15 | December 2026 |
| 6.1, 6.6 | December 2027 |
| 6.12, 6.18 | December 2028 |

The Civil Infrastructure Platform maintains its own super-longterm (SLTS) branches ([CIP](https://cip-project.org/blog/2025/05/26/cip-is-now-supporting-five-slts-kernels)):

| CIP branch | Maintained to |
|---|---|
| 4.4-cip | January 2027 |
| 4.19-cip | about 2029 (uncertain) |
| 5.10-cip | about 2031 |
| 6.1-cip | about 2033 |
| 6.12-cip | about 2035 |

Two consequences for this folder:
- 5.10 and 5.15 leave the kernel.org longterm list three months after this folder is written. They stay in the current set for as long as CIP maintains them.
- 4.4 and 4.19 are ten year old kernels that still receive fixes and run in production. "All versions" is not only an archaeology exercise.

## 1.2 Eras, tags and where to get them

Linus's repository has 948 tags. 114 of them are non-rc releases, from `v2.6.11-tree` and `v2.6.12` to `v7.2`. Git history starts at 2.6.12-rc2 in April 2005. Stable point releases are tagged in `stable/linux.git`, not in Linus's tree.

| Era | First | Last | Where |
|---|---|---|---|
| 0.x | 0.01, 1991-09-17 | 0.99.15 | `pub/linux/kernel/Historic/` |
| 1.x | 1.0, 1994-03-14 | 1.3.100 | `v1.0/` to `v1.3/` |
| 2.0 | 1996-06-09 | 2.0.40, 2004 | `v2.0/` |
| 2.2 | 1999-01 | 2.2.26, 2004 | `v2.2/` |
| 2.4 | 2001-01-04 | 2.4.37.11 (uncertain), 2010 | `v2.4/` |
| 2.6 | 2.6.0, 2003-12 | 2.6.39, 2011-05 | `v2.6/`, and git from 2.6.12 |
| 3.x | 3.0, 2011-07 | 3.19 | `v3.x/` |
| 4.x | 4.0, 2015-04-12 | 4.20 | `v4.x/` |
| 5.x | 5.0, 2019-03-03 | 5.19 | `v5.x/` |
| 6.x | 6.0, 2022-10-02 | 6.19, 2026-02-08 | `v6.x/` |
| 7.x | 7.0, 2026-04-12 | 7.3-rc5 today | `v7.x/` |

Tarballs are at `cdn.kernel.org/pub/linux/kernel/`, and the directory listing was checked. For git history before 2.6.12:
- The grafted full history is best cloned from [mpe/linux-fullhistory](https://github.com/mpe/linux-fullhistory), followed by `git fetch origin 'refs/replace/*:refs/replace/*'`.
- It joins Dave Jones's 0.01 to 2.4.0 import, tglx's BitKeeper `history.git` from 2.4.0 to 2.6.12, and Linus's tree ([LWN 285366](https://lwn.net/Articles/285366/)).

`rucc-kernel` pins tarballs by SHA-256 for every release it grades. It uses the full history clone only for bisection across versions (document 13).

## 1.3 Toolchain minimums by era

This table is built from `Documentation/Changes` (to 4.x) and `Documentation/process/changes.rst`, read at every tag from 2.6.12 to 7.2 **[tree]**, plus `scripts/min-tool-version.sh` from 5.13.

| Kernels | Minimum GCC | Minimum binutils | Minimum Clang |
|---|---|---|---|
| 2.2.0 | 2.7.2.3 | 2.8.1.0.23 | |
| 2.4.0 | 2.91.66 (egcs 1.1.2, and 2.95.2 "may cause problems") | 2.9.1.0.25 | |
| 2.6.0 to 2.6.15 | 2.95.3 | 2.12 | |
| 2.6.16 to 4.18 | 3.2 | 2.12, then 2.20 from 4.13 | |
| 4.19 to 5.7 | 4.6 (`cafa0010cd51`) | 2.20, 2.21 (5.3), 2.23 (5.7) | |
| 5.8 to 5.14 | 4.9 (4.8 in the merge window, 4.9 by 5.8's release) | 2.23 | 10.0.1 from 5.10 |
| 5.15 to 6.14 | 5.1 (`76ae847497bc`) | 2.23, then 2.25 from 6.2 | 11 (5.17), 13.0.1 (6.9) |
| 6.15 | 8.1 on x86, 5.1 elsewhere | 2.25 | 13.0.1 |
| 6.16 to 7.2 | 8.1 everywhere (`118c40b7b503`) | 2.30 | 15.0.0 (6.18), 17.0.1 (7.2, `ce3267a39a92`) |

The minimum make is 3.79.1 at 2.6.0, then 3.80, then 3.81 (4.12), then 3.82 (6.1), then 4.0 (6.11).

The minimum pahole is 1.16 from 5.16, then 1.22 (7.0), then 1.26 (7.2).

The minimum Rust is 1.62 at 6.1 and has moved up to 1.85 at 7.1. It is not relevant to a C compiler except as the reason `CONFIG_RUST` is off.

The Clang 17.0.1 bump in 7.2 is worth reading, because of what it says about GCC ([`ce3267a39a92`](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=ce3267a39a92)). Two of the reasons are behaviours GCC 8.1 already has and the kernel now relies on:
- the scoping of `asm goto` labels against `__cleanup` variables;
- `const` variables accepted where an integer constant expression is required, as in `_Static_assert`.

rucc refuses the second one today (document 03).

## 1.4 The C dialect by era

These dates settle the default dialect rucc must pick when no `-std` flag is given:

| Kernels | What the tree asks for | Effect on rucc |
|---|---|---|
| to 3.17 | nothing | builds got GCC's default dialect, which was gnu89 until GCC 5. rucc's default is C23, so with an old persona it compiles a different language from the one these kernels were written for |
| 3.18 to 5.17 | `-std=gnu89`, added explicitly when GCC 5 changed its default (commit to be confirmed at K0) | |
| 5.18 on | `-std=gnu11` (`e8c07082a810`) | |
| 6.19 | adds `-fms-extensions`, for anonymous members of a tagged struct type **[tree]** | |
| 7.1 on | the flag becomes `-fms-anonymous-structs` where the compiler has it, with `-fms-extensions` as the fallback **[tree]** | |

The kernel has not moved to C23 as of 7.2. GCC 15 switched its own default to gnu23, and that broke every part of the tree that did not pass `-std`: the x86 compressed boot code, the EFI stub, the s390 decompressor and the MIPS vDSO. The stable branches received `-std=gnu11` fixes for it (see the [5.10 fix series](https://patchew.org/MPTCP/20251017-v5-10-gcc-15-v1-0-cdbbfe1a2100@kernel.org/)). An old tag that never got those fixes breaks the same way under rucc's C23 default, which is why document 05 makes the default dialect follow the persona.

## 1.5 Prior art

### TCCBOOT, 2004

Fabrice Bellard's TCC compiled Linux 2.4.26 from source at boot and started it in under 15 seconds on a 2.4 GHz Pentium 4 ([tccboot](https://bellard.org/tcc/tccboot.html)).

It did not use the kernel's Makefiles. It carried a patch (`linux-2.4.26-tcc.patch`) for these problems ([README](https://bellard.org/tcc/tccboot_readme.html)):
- assembler directives TCC lacked (`.rept`, `.subsection`);
- `__ASSEMBLY__` not defined in `.S` files;
- static variables invisible to inline assembly;
- `?:` lvalues;
- `long long` bitfields;
- struct level `aligned`;
- a preprocessor bug.

Bellard's own summary: "there are still many bugs in the kernel generated". The lesson is that the patch list is exactly the list of things a compiler has to have. rucc's rule of no patches turns each item on it into a compiler requirement.

### Claude's C Compiler, February 2026

Anthropic's Rust C compiler was written by parallel Claude agents over about two weeks ([blog](https://www.anthropic.com/engineering/building-c-compiler), [repository](https://github.com/anthropics/claudes-c-compiler)). It built Linux 6.9 for x86-64, AArch64 and RISC-V, and booted it. It identifies as GCC 14.2.0, with the banner `ccc (Claude's C Compiler, GCC-compatible) 14.2.0`.

Its limits, as the blog stated them:
- It had no 16-bit x86 code generator and called GCC for `-m16`.
- At launch it used the GNU assembler and linker. It gained its own later, with cargo features to fall back to GCC's.
- Its generated code was slower than GCC's at `-O0`.

The documented boot (`BUILDING_LINUX.txt`) is RISC-V `defconfig` linked with GNU ld 2.42.

An independent attempt to build x86-64 `defconfig` ([harshanu.space](https://harshanu.space/en/tech/ccc-vs-gcc/)) compiled every C file. It then failed at link time with about 40,000 undefined references, caused by malformed `__jump_table` relocations and `__ksymtab` entries. It also measured SQLite at `-O2` running about 1,200 times slower than GCC's build.

Three lessons for rucc:
- Section and relocation correctness is where a kernel build fails once the C is parsed. That is why document 09's section differential exists.
- `-m16` is where everyone cheats first, which is why the bring-up flag is named for it.
- Booting is not performance, and document 12 is separate for that reason.

### ClangBuiltLinux, 2012 onward

The first attempt was LLVMLinux, from about 2012 to 2015, which lived on out-of-tree patches. ClangBuiltLinux, from about 2018, upstreamed the fixes instead. Its main blocker was `asm goto`: after "x86: Force asm-goto" in 4.17, Clang could not build x86-64 mainline at all until Clang 9 in September 2019 ([release notes](https://releases.llvm.org/9.0.0/tools/clang/docs/ReleaseNotes.html)). Link time optimization came in 5.12 and kCFI in 6.1.

It took a funded team about seven years, with the kernel maintainers adapting the tree to Clang. rucc gets no such adaptation. It has to look like GCC.

### Intel ICC, 2009 to 2023

LinuxDNA built and booted 2.6.22 with ICC in 2009, with patches up to about 2.6.33 and a wrapper that stripped GCC-only flags ([LWN](https://lwn.net/Articles/320795/)). `compiler-intel.h` stayed in the tree until 6.3 removed it (`95207db8166a`). The lesson is that a wrapper stripping flags gets a kernel that is configured wrongly. The `.config` differential exists to catch exactly that.

### Others

Searches found no evidence that any of these builds a modern kernel: Kefir, chibicc, lacc, cproc with QBE, scc, cparser, OpenWatcom. Zig's `cc` is Clang. Sparse is not a code generator, but it is a useful precedent: it defines `__CHECKER__`, and the kernel headers have explicit paths for it.

## 1.6 The size of the thing

These counts come from the 7.2 tarball, taken with `tar t` on 30 September 2026.

| Item | Count |
|---|---|
| files | 101,060 |
| `.c` | 36,927 |
| `.h` | 26,891 |
| `.S` | 1,346 |
| `.rs` | 473 |
| Kconfig files | 1,916 |
| Makefile and Kbuild files | 3,331 |

The tree is about 43 million lines by `wc -l`. About 6 million of those are the AMD GPU driver, mostly generated register headers, which stress the preprocessor and the parser more than anything else. `allmodconfig` on x86-64 builds about 11,000 modules.

GCC build times on current hardware ([Phoronix](https://www.phoronix.com/review/near-10-sec-kernel-build/2)):
- x86-64 `defconfig` is 15 to 30 seconds on a 128 thread machine, and roughly 1 to 2 minutes on 32 threads.
- `allmodconfig` is 2 to 3 minutes on 128 threads, and 15 to 30 minutes on 8 to 16 cores.

## 1.7 Where rucc's own spec already stands

These parts of the parent spec already cover the kernel:
- **Document 14.5, rung 4:**
  - level A: x86-64 `defconfig` boots to a shell in QEMU with no patches and no `objtool` warnings;
  - level B: `allmodconfig` builds;
  - level C: three architectures, selftests and LTP at parity, a userspace built under the kernel, and KASAN, stack protector, retpoline and ORC configurations.
- **Document 13.7** lists the flag set.
- **Document 13.4** lists the attribute pressure points.
- **M10 (#11)** is hardening plus a fourth target. **M11 (#12)** is the kernel, estimated at 4 to 8 months.
- **`rucc-cross`** pins Linux 6.19 for UAPI headers only (`sysroots/manifest:70`). That is `headers_install`, not a kernel build.
- **`rucc-real-corpus`** builds busybox 1.38.0 with kbuild at rung R4. Its `crates/rrc-run/src/kconfig.rs` edits a `.config`. Both are useful here.

This folder is newer than all of these and changes several of their assumptions:
- "All versions" is a claim the parent never made.
- The graded levels are the kernel's own, `-O2` and `-Os`, not the parent's five. The kernel cannot be built at `-O0`, because it relies on dead code elimination for correctness.
- i686 is forced, as document 00 explains.
- The parent's cost of 4 to 8 months covers roughly K0 to K4 of this folder (x86 builds and boots). The whole folder is more like two engineer-years (document 15).
