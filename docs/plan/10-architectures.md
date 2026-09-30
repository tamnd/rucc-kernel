# Architectures

Four rows are graded (02.6). This document states, for each, what rucc must add beyond the common work of documents 05 to 09, and in what order. The order is fixed:
1. X64;
2. A64 right after;
3. X32 forced by X64's own boot code;
4. R64 when rucc's parent milestone M9 delivers a RISC-V back end.

## 10.1 X64: x86-64

The primary row. Documents 07 to 09 are written mostly from its point of view. Summary of its specific work:

| Item | Doc | Milestone |
|---|---|---|
| kernel code model, non-PIC, `R_X86_64_32S`, weak without GOT | 08.2 | K2 |
| general-regs-only | 08.3 | K2 |
| 8-byte stack alignment | 08.4 | K2 |
| `%gs` stack protector with symbol | 08.5 | K2 |
| retpoline, rethunk, SLS, IBT without jump tables | 08.6 | K2/K3 |
| system instructions and operands | 07.4.1 | K2 |
| `__seg_gs` named address space | 03.3 | K5 |
| objtool clean | 09.1 | K3 |
| `-m16` setup and `-m32` vDSO | 10.3 | K4 |
| hardening options, BTF, distro config | 08.6, 09.8 | K5 |

From 2.6.24 on, `ARCH=x86_64` and `ARCH=i386` both build from the merged `arch/x86` tree. Before that, `arch/x86_64` is a separate tree with its own Makefile and flag set. `-mcmodel=kernel` and `-mno-red-zone` are there from the start. `rk flags-diff` records the flags per version, and they need no work beyond the era personas.

## 10.2 A64: AArch64

rucc's AArch64 back end is complete for user programs (TARGETS.md lists it as tier 4 because no suite grades it, but the real corpus builds with it on the Linux arm runners). Kernel specifics:

| Item | Detail | Size |
|---|---|---|
| `-mgeneral-regs-only` | no FP/SIMD registers outside `kernel_neon_begin` | M |
| `-mcmodel=small` with `-fno-pic` | arm64 objects are compiled non-PIC. With `RELOCATABLE` (on in `defconfig`), the image is linked `-pie` (`-shared -Bsymbolic` in older versions) so that the kernel can apply its own `R_AARCH64_RELATIVE` entries at boot. So data carries `ABS64` relocations and code uses `ADRP` plus `ADD` or `LDR` with no GOT. arm64's `vmlinux.lds.S` also asserts on `.got` size (the exact rule varies by version; K6 checks it) | M |
| stack protector via `sp_el0` + offset | 08.5 | M |
| `-mbranch-protection=pac-ret[+leaf][+bti]`, `-msign-return-address=` (old) | `paciasp`/`autiasp` in prologue/epilogue, `bti c` landing pads at address-taken function entries, the GNU property note for BTI | M |
| `-ffixed-x18` / shadow call stack | `SHADOW_CALL_STACK` is Clang-first, GCC 12+ supports `-fsanitize=shadow-call-stack` on arm64. Not graded (08.10) but x18 must be reserved when `-ffixed-x18` is passed (it is not in defconfig) | S |
| `-mno-outline-atomics` | the kernel passes it so atomics are inline, never `__aarch64_ldadd4_acq` helpers | S. Check rucc's default for `-march=armv8-a` atomics |
| `-march=armv8-a+...`, `-Wa,-march=armv8.x-a` | the assembler must accept the kernel's extension set via `.arch`/`.arch_extension` | 07.4.2 |
| system registers and instructions | 07.4.2 | L |
| `-fpatchable-function-entry=2` ftrace | done |, |
| `KASAN` generic/sw-tags/hw-tags | not graded |, |
| `#pragma GCC visibility push(hidden)` | EFI stub and `arch/arm64/kernel/pi/` use it (6.x) | S, check |
| `-mabi=lp64`, big-endian | `CPU_BIG_ENDIAN` is a supported arm64 config; rucc has no `aarch64_be`. Not graded, compile-only if ever |, |
| `__attribute__((target("arch=...")))` / `target("+lse")` | some crypto and `lse.h` paths | S |

The arm64 inline assembly surface is smaller than x86's per file but the system register table is large (07.4.2). The kernel's `alternative_cb`/`ALTERNATIVE` macros in `asm/alternative-macros.h` exercise the macro processor exactly like x86's.

**Boot:** `qemu-system-aarch64 -M virt -cpu max -kernel Image -initrd ... -append "console=ttyAMA0"`. No firmware needed (QEMU loads `Image` directly). `-cpu max` gives PAC/BTI/MTE which tests the pac-ret path; a second run with `-cpu cortex-a57` tests the non-PAC fallback. KVM on arm runners: open question 8.

**Cost:** 6 weeks at K6, after K2 to K5's shared work.

## 10.3 X32: i686

Forced by X64: the x86-64 kernel builds 16-bit real mode setup code with `-m16` (5.11+: GCC's `-m16` is i386 codegen emitting `.code16gcc`) and the 32-bit vDSO with `-m32` under `IA32_EMULATION` (defconfig). Without an i686 back end rucc cannot build an x86-64 `bzImage` unmodified. It is also the museum's only architecture. This settles the M10 fourth-target question in rucc's spec: i686 is the fourth target.

| Item | Detail | Size |
|---|---|---|
| i686 back end | 8 general registers; `cdecl`, and `regparm(3)` because the i386 kernel is built `-mregparm=3` throughout and `asmlinkage` means `regparm(0)`; 64-bit arithmetic in register pairs. The kernel does not link libgcc, so a `u64 / u32` written in C fails to link with GCC as well, and the kernel uses `do_div()` instead. rucc must never *introduce* a `__udivdi3`, `__umoddi3`, `__divdi3`, `__ashldi3` or similar call that the source did not ask for, for example through strength reduction or loop idiom recognition | XL |
| x87 and SSE off | i386 kernel: `-mno-sse -mno-mmx -mno-80387` (the `-msoft-float` era) | with 08.3 |
| `-mpreferred-stack-boundary=2` | 4-byte alignment | with 08.4 |
| stack protector `%fs:__stack_chk_guard` (5.x+), `%gs:20` (older) | 08.5 | S after x86-64 |
| `-m16` | i386 codegen, all instructions emitted under `.code16gcc` (the assembler adds operand/address size prefixes to make 32-bit code run in 16-bit mode), so the compiler is just i686 with `-march=i386`, and the assembler does the work | M on the assembler |
| `.code16` hand-written real mode asm | `arch/x86/boot/*.S`, `realmode/rm/*.S`: 16-bit encoding (different default operand size, ModRM 16-bit addressing forms `(%bx,%si)`) | M to L |
| `-march=i386`/`i486`/`i586`/`pentium`/... | per `CONFIG_M*` CPU choice. The instruction subset: no `cmov` below i686, no `cmpxchg8b` below i586, `bswap` from i486. rucc must honor `-march` for these | M |
| `R_386_*` relocations, `-fno-pic` default | object writer | M |
| `-mcmodel` n/a; kernel at `0xC0000000` with absolute addressing | natural in i386 non-PIC |, |
| `regparm` and `stdcall`/`fastcall` attributes | `fastcall` was used in 2.6.early | S after regparm |
| a.out object output | museum 1.x (and 2.0 option) | M, K11 |

Rows: X32 boots `i386` defconfig (`ARCH=i386 defconfig` → `i386_defconfig`) under `qemu-system-i386`. The `-m16` part is tested by every X64 and X32 boot.

**Cost:** 8 weeks at K4 (a new back end reusing x86-64 instruction selection tables, the register allocator with a smaller file, and 64-bit lowering). The rucc spec's M10 estimated the fourth target in the same range.

## 10.4 R64: riscv64

Blocked on rucc's M9 (the RISC-V back end, parent issue #10). When it exists, kernel specifics:

| Item | Detail |
|---|---|
| `-mcmodel=medany`, `-mno-relax` or relaxation | the kernel supports linker relaxation (GNU ld and lld); the compiler must emit `R_RISCV_RELAX` pairs correctly or pass `-mno-relax` behaviour. The CCC project's riscv boot used GNU ld; relaxation correctness is the main risk |
| `-march=rv64imac_zicsr_zifencei` (+`_zba_zbb` etc. via alternatives) | the `.option arch,+zbb` directive in inline asm (6.x) |
| `-mno-save-restore`, `-mstrict-align` | |
| `-fno-omit-frame-pointer`, `-mabi=lp64` (soft-float ABI, no FPU in kernel) | `lp64` rather than `lp64d` |
| `tp` global register, `gp` relaxation off in modules | 07.3 |
| stack protector `tp`+offset | 08.5 |
| `.insn`, `.option push/pop/norvc/arch`, `%pcrel_hi`/`%pcrel_lo`, `%hi`/`%lo` | assembler |
| SBI: boot via OpenSBI in QEMU `-M virt -bios default` | |

**Cost:** 12 weeks at K7 including the kernel parts of the back end; the back end itself is counted in rucc's M9, not here, and K7 waits for it.

## 10.5 Compile-only architectures

`rk build --arch=<a>` works for any `ARCH` whose target rucc has. Today that means nothing beyond the four rows. If rucc gains another back end (loongarch64, powerpc64le and s390x are the usual candidates), its kernel row appears in the report as compile-only. It becomes graded only by a new decision recorded in `rows.toml` with a QEMU machine and a reference toolchain.
