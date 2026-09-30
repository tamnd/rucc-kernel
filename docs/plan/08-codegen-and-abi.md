# Code generation and the kernel ABI

A user program and a kernel are compiled under different contracts. This document states the kernel's contract per architecture, what rucc must do to meet it, and how each item is checked.

## 8.1 The contract, x86-64

These are the flags from 7.2 `arch/x86/Makefile` and the top-level Makefile for `defconfig`, with what each demands of rucc.

| Flag | Meaning | rucc work |
|---|---|---|
| `-mcmodel=kernel` | code and static data in the top 2 GB, so addresses are sign-extended 32-bit absolutes | 8.2 |
| `-fno-PIE`, `-fno-pic` | no GOT, no PLT, direct absolute and PC-relative addressing | 8.2 |
| `-mno-red-zone` | interrupts use the current stack, so nothing below `%rsp` | done |
| `-mno-sse -mno-mmx -mno-sse2 -mno-3dnow -mno-avx -mno-80387 -mno-fp-ret-in-387` (and `-mgeneral-regs-only` on newer GCC) | no vector or x87 register may be touched | 8.3 |
| `-mskip-rax-setup` | do not set `%al` for variadic calls (no vector args exist) | S: a code size item, not a correctness one |
| `-mpreferred-stack-boundary=3` (or `-mstack-alignment=8` in Clang terms) | 8-byte stack alignment | 8.4 |
| `-falign-jumps=1 -falign-loops=1` | no padding | accept, and honor as no padding |
| `-fno-asynchronous-unwind-tables` | no `.eh_frame` from C (ORC replaces it) | done |
| `-mindirect-branch=thunk-extern -mindirect-branch-register` | retpoline | 8.6 |
| `-mindirect-branch-cs-prefix` | pad indirect thunk calls for rewriting | 8.6 |
| `-mfunction-return=thunk-extern` | return thunk | 8.6 |
| `-mharden-sls=all` | `int3` after `ret` and indirect `jmp` | 8.6 |
| `-fcf-protection=branch` (IBT), `-fno-jump-tables` with it | `endbr64` at every address-taken function, no indirect jumps via tables | done for `endbr`, jump tables 8.6 |
| `-mstack-protector-guard-reg=gs -mstack-protector-guard-symbol=__ref_stack_chk_guard` (6.x); `-mstack-protector-guard=tls` with `%gs:40` (older) | canary location | 8.5 |
| `-mfentry -mrecord-mcount` or `-fpatchable-function-entry=` via `CC_USING_*` | ftrace | 8.7 |
| `-fno-delete-null-pointer-checks`, `-fno-strict-overflow`, `-fno-strict-aliasing`, `-fno-allow-store-data-races`, `-fconserve-stack`, `-fno-stack-clash-protection`, `-fzero-init-padding-bits=all` | semantics | 06.6, and `-fconserve-stack` is an inlining heuristic (8.8) |
| `-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer` (with `FRAME_POINTER=y`); ORC builds allow omission | frames | 8.8 |
| `-fmin-function-alignment=16` or `-falign-functions=16` and `CONFIG_FUNCTION_PADDING_BYTES` via `-fpatchable-function-entry=16,16` | call depth tracking, FineIBT | 8.7 |
| `-fzero-call-used-regs=used-gpr` | hardening | 8.6 |
| `-ftrivial-auto-var-init=zero` | hardening, default on in many distro configs | 8.6 |
| `-fstrict-flex-arrays=3` | only true flexible arrays are unbounded, which matters for `__builtin_object_size` and FORTIFY | S to M |
| `-fsanitize=*` | KASAN, KCSAN, UBSAN, KCOV, KMSAN | not in this plan's grade (8.10) |

## 8.2 The kernel code model

With `-mcmodel=kernel` the image is linked at `0xffffffff80000000` or above. Every symbol's address fits in a sign-extended 32-bit immediate. So:
- **Direct data access** is `mov sym(%rip), %reg`, which is `R_X86_64_PC32` exactly as in the small model, or `mov sym, %reg` / `mov sym(,%idx,8), %reg` with `R_X86_64_32S`. The second form is how GCC indexes static arrays: one instruction instead of `lea` plus index.
- **Address materialization** is `mov $sym, %reg` (`R_X86_64_32S`, 7 bytes), or `lea sym(%rip), %reg`. GCC prefers `mov $sym` in kernel model.
- **Function calls** are `call sym`, with `R_X86_64_PLT32` from binutils 2.31, `PC32` before. There is no PLT, since the linker resolves them directly.
- **Weak undefined symbols** must resolve to 0. They are referenced as `$sym` or `PC32`, and `ld` in `-no-pie` mode resolves `PC32` to an undefined weak symbol as `0 - P`, which the kernel tolerates. **Never** through `GOTPCREL`, which creates a `.got` entry: `arch/x86/kernel/vmlinux.lds.S` asserts `SIZEOF(.got) == 0` (check the version), and ld otherwise makes a GOT the kernel never relocates.
- **Percpu.** `%gs:sym` (`R_X86_64_32S` against a percpu symbol, since `.data..percpu` is linked at 0 in older kernels) and `%gs:sym(%rip)` in 6.x (PC-relative percpu with `CONFIG_SMP`). The form depends on the kernel version, which picks it with `__percpu_seg_override` and the `__seg_gs` address space (6.9+). Either way, rucc's inline asm operands must produce exactly these.

What rucc must add:
1. A code model enum in the x86-64 target, with `small` (the current default), `kernel`, and later `medium`/`large` if wanted. Kernel mode changes address materialization, array indexing and the relocation types listed above.
2. A non-PIC mode (`-fno-pic`), where extern and weak data are accessed directly, not through the GOT. rucc today assumes PIE for executables and accesses extern data via `GOTPCREL`, relaxed by the linker. Relaxation is not enough here, because weak undefined symbols cannot be relaxed and leave a `.got`.
3. `R_X86_64_32S` in the object writer (`crates/rucc-object/src/elf.rs:39`).
4. A check, in rucc's own test suite, that a kernel-model object contains no `GOTPCREL`, `GOTPCRELX`, `REX_GOTPCRELX`, `PLT` stub or TLS relocation. It is a rucc-corpus facet, `mcmodel-kernel`.

**L, K2.** The x86-64 back end's address mode selection is the main change. The i686 back end (K4) gets non-PIC as its default.

The boot decompressor (`arch/x86/boot/compressed/`) is built `-fPIE` (from 5.x) or `-fPIC` and is position independent, with its own `.got` assertions. Some versions demand hidden visibility for everything (`-include hidden.h`) so that no GOT is needed. rucc's PIE mode must then produce no GOT entries for hidden symbols, which it does today.

## 8.3 No FPU, no vectors

The kernel may use the FPU only between `kernel_fpu_begin()` and `kernel_fpu_end()`, in files compiled with `CFLAGS_REMOVE` of the no-FPU flags plus `-msse2` and friends (for example `lib/raid6/`, `arch/x86/crypto/`, `drivers/gpu/drm/amd/display/dc/dml/`). Everywhere else, a single use of `%xmm0` corrupts user state.

rucc on x86-64 uses SSE registers for:
- `float` and `double`, which the kernel does not use outside FPU regions;
- **struct copies and `memset`/`memcpy` expansions**, the dangerous case;
- vectorized loops if rucc vectorizes, check;
- **passing and returning** `float` and `double` per the ABI;
- `va_start` in variadic functions (the register save area stores `%xmm0`-`%xmm7` when `%al != 0`).

`-mgeneral-regs-only`, or the set `-mno-sse -mno-mmx -mno-80387`, must:
1. Remove all vector and x87 registers from allocation.
2. Expand copies and fills with general registers only (`rep movsb` or `movq` sequences).
3. Make any `float` or `double` operation a hard error, as GCC does ("SSE register return with SSE disabled"), except when only types are declared.
4. Omit the vector part of the variadic register save area.
5. Refuse vector inline asm constraints (`x`, `v`, `Yz`).

The arm64 kernel uses `-mgeneral-regs-only` in the same way, with NEON in `kernel_neon_begin` regions only.

Check: `rk vec-audit` disassembles every object in the rucc build that the reference build's flags mark as no-FPU and fails on any vector or x87 instruction. It is cheap, total, and runs from K2. **M, K2** for x86-64, and arm64 at K6.

## 8.4 Stack alignment

x86-64 kernels use `-mpreferred-stack-boundary=3`, which is 8-byte alignment, because interrupt entry does not align the stack and the psABI's 16-byte promise does not hold on kernel stacks. The consequences for rucc:
- Frames are not padded to 16.
- A local variable with `__aligned(16)` or larger must be aligned dynamically (`and $-16, %rsp` with a frame pointer), as GCC does.
- `movaps` to the stack is never used, and it cannot be under 8.3 anyway.

i386 uses `=2`, which is 4 bytes. **M, K2.**

## 8.5 Stack protector

| Arch and version | Guard |
|---|---|
| x86-64 before 6.10 (approx., check) | `%gs:40`: the canary sits at offset 40 of the per-CPU `fixed_percpu_data` |
| x86-64 6.10+ | `%gs:__stack_chk_guard` via `-mstack-protector-guard-reg=gs -mstack-protector-guard-symbol=__ref_stack_chk_guard` |
| i386 | `%fs:__stack_chk_guard` via `-mstack-protector-guard-reg=fs -mstack-protector-guard-symbol=__stack_chk_guard`, or `%gs:20` before 5.x (lazy gs) |
| arm64 | `-mstack-protector-guard=sysreg -mstack-protector-guard-reg=sp_el0 -mstack-protector-guard-offset=<offset of stack_canary in task_struct>`, with `CONFIG_STACKPROTECTOR_PER_TASK` |
| riscv | `-mstack-protector-guard=tls -mstack-protector-guard-reg=tp -mstack-protector-guard-offset=<...>` |

rucc today hard-codes `%fs:40` on x86-64 and refuses stack protection on arm64. The fix is the four `-mstack-protector-guard*` flags with GCC's semantics, plus `-fstack-protector`, `-fstack-protector-strong` and `-fstack-protector-all` heuristics matching GCC's choice of protected functions. The last matters for 12's stack size comparison and for objtool, since `__stack_chk_fail` is a noreturn call objtool must know.

`no_stack_protector` and `__attribute__((optimize("-fno-stack-protector")))` must work (#811). Early boot and the CPU bring-up paths use them to avoid a canary check across the point where the canary is set.

**M, K5**, because `defconfig` enables `STACKPROTECTOR_STRONG`. Until then, the probe of 05.3 fails identically for rucc on both sides only if the reference also lacks it, which it does not. So `.config` differs, and **K3 needs at least `-mstack-protector-guard-reg=gs` with the symbol form** for 7.2 `defconfig` to have identical `.config`. We move that part of the item forward into K2 and leave arm64 at K6.

## 8.6 Speculation mitigations and hardening

| Mitigation | Compiler part | Validation |
|---|---|---|
| retpoline (4.15+) | every indirect call and jump through a register becomes `call __x86_indirect_thunk_<reg>` / `jmp __x86_indirect_thunk_<reg>`, with `-mindirect-branch-register`. No indirect branch through memory. Jump tables are forbidden, since they are indirect jumps. The thunks are provided by `arch/x86/lib/retpoline.S`, which is external | objtool: "indirect call found in RETPOLINE build" / `.retpoline_sites` complete |
| cs-prefix (6.0+) | a `cs` prefix on the thunk call so it can be rewritten in place to a 6-byte `lfence; call *%reg` | the section differential of 09.4 |
| return thunk (5.19+) | every `ret` becomes `jmp __x86_return_thunk` | objtool: "'naked' return found in RETHUNK build" |
| SLS (5.17+) | `int3` after every `ret` and every indirect `jmp` | objtool: "missing int3 after ret" |
| IBT (5.18+) | `endbr64` at the entry of every function whose address escapes, plus `notrack` never used | objtool `--ibt`: "missing ENDBR" |
| FineIBT, CFI (6.2+) | kCFI is Clang-only (`-fsanitize=kcfi`), and GCC 16 has no kCFI either | not claimed. `CFI_CLANG` off on both |
| call depth tracking (6.2+) | `-fpatchable-function-entry` padding plus the objtool call site list | objtool |
| zero-call-used-regs (5.15+) | zero caller-saved registers before `ret` per `used-gpr` | `rk` checks the epilogue pattern on a sample |
| trivial auto-var-init (5.9+) | zero every uninitialized automatic, including padding | KUnit `stackinit_kunit`, which is exactly the test for this |
| FORTIFY_SOURCE | `__builtin_object_size`, `__builtin_dynamic_object_size`, `__attribute__((error))`, `__builtin_constant_p` after inlining | KUnit `fortify_kunit`, `rk` link check for `__read_overflow*`, and 06.4 |
| `-fstrict-flex-arrays=3` | changes `__builtin_object_size` on trailing arrays | KUnit `fortify_kunit`, `overflow_kunit` |

rucc must implement the compiler part of each. All are local rewrites in the x86-64 back end at the call, jump and return lowering, and all are **M** at most, apart from forbidding jump tables (**S**) and matching GCC's endbr placement (**S**, rucc already emits `endbr64`).

All are **K5**, but several are on in `defconfig`:
- `MITIGATION_RETPOLINE`;
- `MITIGATION_RETHUNK`;
- `MITIGATION_SLS` (x86 `defconfig` has had it on since 5.17 when the compiler supports it);
- `X86_KERNEL_IBT`.

If rucc lacks them, the `.config` differs at K3, since the probes answer no. **K3's exit needs them**, or K3's grade is on a different kernel. The resolution: K2 and K3 implement retpoline, return thunk, SLS and the IBT jump table rule for x86-64 (about 3 weeks). K5 does the remaining hardening options, which are off in `defconfig` and on in distro configs.

## 8.7 Function entry: ftrace, patchable entries, alignment

| Kernel | Mechanism | rucc |
|---|---|---|
| x86-64 2.6.x to 4.x | `-pg` gives `call mcount` after the frame setup. `-mfentry` (4.x+) gives `call __fentry__` as the first instruction. `recordmcount.pl`/`.c` or `-mrecord-mcount` collects call sites into `__mcount_loc` | `-mfentry` and `-pg` done on x86-64. `-mrecord-mcount` missing. Without it, `recordmcount` (a host tool) scans the object, which works if the call relocations look like GCC's |
| x86-64 5.x+ | objtool `--mcount` creates `__mcount_loc` (5.12+), `-mnop-mcount` optional | objtool reads the `call __fentry__` relocation. Must be a direct `call` with `PLT32` or `PC32` |
| x86-64 6.x | `-fpatchable-function-entry=16,16` for call depth tracking or `FUNCTION_PADDING` | done |
| arm64 | `-fpatchable-function-entry=2` (5.5+), older used `-pg` | patchable entry done, `-pg` refused (fine, only for pre-5.5) |
| riscv | `-fpatchable-function-entry=8` or `4` | with K7 |

Function alignment (`-falign-functions=`, `-fmin-function-alignment=`) must be honored exactly, since `CONFIG_FUNCTION_ALIGNMENT` is checked by objtool and relied on by the call thunks.

**S to M, K3 and K5.**

## 8.8 Frames and stack use

Kernel stacks are 16 KB on x86-64 (8 KB before 3.15), 16 KB on arm64 (plus a separate IRQ stack), and 8 KB or 4 KB on i386 (`4KSTACKS` in 2.6). A rucc frame that is 2x GCC's does not fail a test. It overflows the stack under a deep call chain, which is exactly what CI does not reproduce.

`CONFIG_FRAME_WARN` (2048 on 64-bit) turns `-Wframe-larger-than=` into warnings, but rucc emits no warnings (#485). So instead:
- **`rk frames`** reads `-fstack-usage` output (`.su` files), which rucc writes today, and compares each function's frame with GCC's.
- A function over `FRAME_WARN` in rucc and not in GCC is a failure.
- The distribution (p50, p99, max, sum over all functions weighted by static call depth) is a report item.
- The graded limit is in 02.5: stack high water mark within 10% of GCC's.

The main contributors to large frames in compilers:
1. **Stack slot sharing.** Locals in disjoint scopes should share slots. GCC does this aggressively, and the kernel depends on it (big `switch` statements with a large struct per case in `ioctl` handlers).
2. **Inlining.** A large `always_inline` or inlined static adds its frame to the caller's. GCC's `-fconserve-stack` limits stack growth from inlining to 100%, via `--param large-stack-frame-growth=100` and `large-stack-frame=256`. rucc must implement the same limit under `-fconserve-stack`.
3. **Spill slots.** The allocator's spill slot coloring (#1177's allocator claims this, check).

**M, K3** for the measurement, and the fixes follow from what it finds.

## 8.9 The memory model and code shape

Document 06.7 lists the semantics. In code generation they mean:
- a `volatile` access of 1, 2, 4 or 8 bytes is exactly one `mov` of that width;
- no store is widened, merged with a neighbour or invented;
- no `cmov` replaces a branch where one arm contains a `volatile` access;
- no load is speculatively hoisted above a branch into a `volatile` context.

`rk` does not test these. The rucc-corpus `lkmm` facet does, before K3.

## 8.10 What is not graded

- **Sanitizers.** KASAN, KCSAN, KMSAN, UBSAN, KCOV, kCFI and shadow call stack need compiler instrumentation rucc does not have. Each is **XL**. They stay off in both builds, and a configuration enabling them is not claimed.
  - `defconfig` has none of them.
  - Debian's config enables KFENCE (no compiler support needed) and UBSAN bounds on some versions. If the K5 distro config has UBSAN on, that configuration is compile-only until rucc has `-fsanitize=bounds`. That is open question 9.
- **GCC plugins** (05.4).
- **LTO**, which is Clang-only in the kernel (`LTO_CLANG`).
- **`-O3`.** `CC_OPTIMIZE_FOR_PERFORMANCE_O3` exists only for ARC and a short period elsewhere.
