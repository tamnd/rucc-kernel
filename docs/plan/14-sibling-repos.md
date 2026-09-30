# Sibling repositories

The kernel work touches every repository in the rucc family. The repository rule (document 00) is:
- fixes go to `rucc`;
- tools go to `rucc-kernel`;
- reductions go to the corpora.

This document lists what each one gains, and when.

## 14.1 `tamnd/rucc`

All compiler work in documents 03 and 05 to 10. Organized as:

| Area | Parent milestone in rucc's spec | Issues to open at K0 |
|---|---|---|
| driver identity and kbuild surface (05.7) | M11 #12 level A | persona banner and std; `-Wa` allow-list; `-fgnu-as-version`; no-op flags table; target by program name; `-print-file-name=plugin`; `--print-features` |
| front end (06) | M11 | null pointer constant ICE; `const` ICE in GNU mode; `section` attribute (#909 exists); `error`/`warning` attribute; `__builtin_constant_p` contract (#1768, #1849 exist); `no_stack_protector` (#811 exists); `__has_attribute` accuracy |
| assembler (07) | M11, with the M7 assembler work | `.S` preprocessing trio; expression comparisons and relaxation; macro processor; per-object assembler stream; `asm goto` (#221 exists); x86 system instruction set; absolute relocations; notes, groups, retain; arm64 system set |
| codegen (08) | M10 #11 hardening, M11 | kernel code model; non-PIC; general-regs-only; stack alignment; stack protector guard flags; retpoline, rethunk, SLS, jump tables; zero-call-used-regs; auto-var-init; `-mrecord-mcount`; `-fconserve-stack`; inline small memcpy |
| targets (10) | M10 (i686 as the fourth target), M9 (riscv64) | i686 back end with `regparm`, `-m16`, `.code16gcc`; arm64 kernel flags |
| debug info (09.8) | M8 #9 | DWARF 4; `-gz` refusal or support |

Two changes to rucc's own spec follow from this folder, and should be proposed as spec edits at K0:
1. **M10's fourth target is i686.** The x86-64 kernel cannot be built without it (10.3), and the museum needs it.
2. **M11 #12's levels A, B and C are redefined** in terms of this folder's milestones:
   - level A = K3 (x86-64 `defconfig` boots, KUnit parity, objtool clean);
   - level B = K5 (hardening, distro config, allmodconfig, kselftest and LTP);
   - level C = K8 (performance).

   The graded optimization levels for the kernel are `-O2` and `-Os` only (02.3), which is an exception to the parent spec's "every level" rule that must be written down there.

Plus the stale metadata fixes of 03.8, which cost a day.

## 14.2 `tamnd/rucc-compat`

Differential corpora, meaning comparisons of rucc against GCC on the same input:

| Corpus | Contents | From |
|---|---|---|
| `kernel-pp` | `-E` output differential for about 200 units per graded version, plus every exporting unit for CRC checks (06.8) | K0 |
| `kernel-asm` | reference `.s` files (from GCC `-S` of C units, and preprocessed `.S`), assembled by gas and by rucc, compared per section (07.6). Stored as a generator plus manifest, not as files | K1 (generator), K2 (graded) |
| `kernel-abi` | UAPI struct layouts (`usr/include` after `make headers_install`), checked by compiling `offsetof`/`sizeof` probes for every struct with both compilers | K1 |
| `kernel-probes` | the kbuild probe list per version (05.3) as a standalone differential. `rk probes` output is imported | K0 |

`rucc-compat` already runs the `-E` and ABI differentials for user headers, so `kernel-pp` and `kernel-abi` reuse its drivers.

## 14.3 `tamnd/rucc-corpus`

Reduced tests, each a small program with an expected result or pattern. New facets:

| Facet | Tests | From |
|---|---|---|
| `null-pointer-constant` | `__is_constexpr` and its variations | K1 |
| `const-ice` | 06.3 | K1 |
| `section-attr` | variables and functions in named sections: alignment, section flags, `__start_`/`__stop_` walks, `used`, `retain` | K1 |
| `constant-p-after-inline` | 50 shapes from 06.4, with checks for no undefined `__bad_*`/`__compiletime_assert_*`/`__*_overflow` references | K2 |
| `asm-goto` | templates with bodies, outputs, multiple labels, in loops, after inlining | K2 |
| `asm-local-labels` | numeric labels across templates and functions, with `.pushsection` | K2 |
| `gas-macros` | the macro processor, with expected objects produced by gas at corpus creation time | K2 |
| `mcmodel-kernel` | relocation type checks: no GOT/PLT/TLS; `32S` forms; weak undefined | K2 |
| `general-regs-only` | struct copies, `memset`, variadic functions, `float` rejection | K2 |
| `objtool-shapes` | one object per objtool bucket found at K2/K3, with objtool from the pinned kernel as the checker | K3 |
| `lkmm` | 06.7 | K3 |
| `frame-size` | functions with disjoint-scope locals, large inlined callees; `.su` expectations vs GCC | K3 |
| `mitigations` | retpoline, rethunk, SLS, IBT, zero-call-used-regs patterns | K3, K5 |
| `i386-kernel` | `regparm`, `-m16` sequences, no introduced libgcc calls | K4 |
| `old-gnu` | cast-as-lvalue, `?:` lvalue, multi-line strings, gnu89 inline, `-traditional` `.S` | K10, K11 |

Each facet's tests carry the kernel version and file that motivated them, as a comment, never as copied code (13.1).

## 14.4 `tamnd/rucc-real-corpus`

- busybox 1.38.0 is already there and is the initramfs userland (11.3.1). Its row stays as it is. `rucc-kernel` consumes the built binary by hash from `rucc-real-corpus`'s artifacts rather than rebuilding it.
- The kernel stays **out** of `rucc-real-corpus`. It is the parent spec's ladder rung 4, graded here with its own harness, as Postgres has `rucc-postgres`.
- Add **LTP** and **kselftest** as real-corpus projects built as user programs, from K5. They are large C test suites that rucc must build anyway, and building them in the real corpus first separates "rucc can't compile LTP" from "rucc's kernel fails LTP".
- Add **`util-linux`** and **`kmod`** as projects, since `modprobe` in the initramfs is busybox's today. A kmod built by rucc is a later, optional step.

## 14.5 `tamnd/rucc-cross`

- It already pins linux 6.19 for UAPI headers in sysroots. Keep it, and update it on the parent spec's schedule, not the kernel's.
- Add binutils per era per target for the reference toolchains, **only if** the era containers (04.4) cannot provide them from Debian archives. Wheezy onward has `binutils-aarch64-linux-gnu`. For the era binutils built from source (E0 to E2), the build recipes go in `rucc-kernel/provision/eras/`, not in `rucc-cross`, since they serve only the kernel.
- The target sysroots `rucc-cross` builds (musl, glibc) serve kselftest and LTP cross builds.

## 14.6 `tamnd/rucc-postgres`

Nothing changes in it. Shared code between `rpg` and `rk` (the shim's JSONL schema, the SSH agent, the report writer, the statistics for benchmarks) is copied at K0. It is factored into a shared crate only when a third harness needs it. Two copies are cheaper than an early abstraction.
