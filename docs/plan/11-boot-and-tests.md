# Boot and tests

A kernel build that links proves little. This document defines the boot rig, the test fragment, the test suites per version set, and how a failure is attributed to rucc.

## 11.1 The test fragment

`rucc-kernel/configs/test.fragment` is merged into every graded configuration. It only turns things on (02.1), and its contents are fixed per era. The fragment for E10 and E11:

```
# console and init
CONFIG_SERIAL_8250=y
CONFIG_SERIAL_8250_CONSOLE=y
CONFIG_SERIAL_AMBA_PL011=y             # arm64 virt
CONFIG_SERIAL_AMBA_PL011_CONSOLE=y
CONFIG_DEVTMPFS=y
CONFIG_DEVTMPFS_MOUNT=y
CONFIG_BLK_DEV_INITRD=y
CONFIG_RD_GZIP=y
CONFIG_TMPFS=y
CONFIG_PROC_FS=y
CONFIG_SYSFS=y
# virtio for disk, net and 9p
CONFIG_VIRTIO=y
CONFIG_VIRTIO_PCI=y
CONFIG_VIRTIO_MMIO=y
CONFIG_VIRTIO_BLK=y
CONFIG_VIRTIO_NET=y
CONFIG_VIRTIO_CONSOLE=y
CONFIG_NET_9P=y
CONFIG_NET_9P_VIRTIO=y
CONFIG_9P_FS=y
# test infrastructure
CONFIG_KUNIT=y
CONFIG_KUNIT_ALL_TESTS=m               # built as modules, loaded by rk-init
CONFIG_DEBUG_STACK_USAGE=y             # 02.5 stack high water mark
CONFIG_PRINTK_TIME=y
CONFIG_PANIC_ON_OOPS=y
# x86-64 only: direct vmlinux boot (09.6)
CONFIG_PVH=y
CONFIG_HYPERVISOR_GUEST=y
CONFIG_PARAVIRT=y
```

Notes:
- **`KUNIT_ALL_TESTS=m` builds every KUnit suite the configuration allows.** It is legal because it only turns things on, and it is the largest single source of tests. It also means `defconfig` plus the fragment compiles a few hundred more units than `defconfig`, which we want.
- **`PANIC_ON_OOPS` makes an oops end the run** with a clear marker rather than a hang.
- **`CONFIG_PVH` and `HYPERVISOR_GUEST`** are for 09.6's direct boot. They are on for both builds, so they are fair.
- **Older eras' fragments** drop what does not exist:
  - KUnit before 5.5;
  - devtmpfs before 2.6.32, where `rk-init` then creates `/dev/console` and `/dev/ttyS0` with `mknod`;
  - virtio before 2.6.24;
  - PL011 console on arm64 is always there.

  `rk` applies the era's fragment and verifies after `olddefconfig` that every requested option took effect, and records which did not. The reference's result is identical by 05.2.

## 11.2 Grading per set

| Set | Build | Boot | Smoke | KUnit | kselftest | LTP | dmesg subset | objtool | Performance |
|---|---|---|---|---|---|---|---|---|---|
| Current, X64 and A64 | defconfig, tinyconfig, allmodconfig (build only), distro (from K5) | defconfig, distro | yes | all built suites | pinned collections (11.5) | pinned runtest (11.5) | yes | X64 | K8, X64 on server3 |
| Current, X32 | i386 defconfig | yes | yes | yes | no | no | yes | X32 from 4.6 | no |
| Current, R64 | defconfig | yes | yes | yes | no | no | yes | no | no |
| Releases | defconfig | yes | yes | from 5.5 | no | no | yes | from 4.6 | no |
| Last points | defconfig | yes | yes | if present | no | no | yes | if present | no |
| Museum | the era's default config (`make config` with defaults: `yes "" \| make oldconfig`) | yes, to `rk-init` | minimal | no | no | no | panic-free | no | no |

The grading follows 02.3: a unit that passes 3 of 3 with the reference must pass with rucc.

## 11.3 The boot rig (`rk boot`)

```
qemu-system-x86_64 -machine q35,accel=kvm:tcg -cpu host|max -smp 4 -m 2G \
  -kernel <bzImage|vmlinux> -initrd <rk-initramfs.cpio.gz> \
  -append "console=ttyS0 panic=-1 oops=panic rk.suite=<s> rk.seed=<n>" \
  -nographic -no-reboot \
  -virtfs local,path=<results>,mount_tag=rk,security_model=none \
  -device virtio-rng-pci
```

- **arm64:** `-machine virt -cpu max`, console `ttyAMA0`.
- **i386:** `-machine pc -cpu qemu32` or `pentium3` to match `CONFIG_M*`.
- **riscv64:** `-machine virt -bios default`.
- **Timeout:** each boot has a wall-clock budget of 3x the reference's median for the same suite, plus 60 s. A timeout is a failure; there is no retry (02.7).
- **Result channel:** results go through 9p when the kernel has it, and over the serial console otherwise, framed as `RK-BEGIN <suite>` / `RK-END <suite> <status>` lines. Old kernels have no 9p, and all results then come through serial.
- **Initramfs:** from 2.6 onward, a cpio initramfs; for 2.4 and earlier, an initrd (ext2 image). 1.x uses a floppy or initrd image and a minix root, see K11.

### 11.3.1 The initramfs

Built by `rucc-kernel/initramfs/`, containing:
- `rk-init`, a static C program (11.6);
- busybox 1.38.0 static, built by **rucc** against musl. It is already in `rucc-real-corpus`, so a busybox failure is caught there first;
- the kselftest and LTP binaries for the row, also built by rucc (5.5, from K5), each with its own build log;
- a GCC-built copy of the same userland, used when a rucc-built userland fails, to attribute the failure (11.7).

The initramfs is pinned by hash per row. Its contents are identical for both kernels in a comparison, so the kernels are the only difference.

## 11.4 KUnit

KUnit suites run inside the booted kernel at module load (`KUNIT_ALL_TESTS=m`) or at boot (`=y` in older kernels). `rk-init` loads each test module in turn with `modprobe`, and the TAP output from `/sys/kernel/debug/kunit/*/results` or dmesg is parsed per suite by the same parser as `tools/testing/kunit/kunit_parser.py`, reimplemented in `rk-tap`.

A suite whose module fails to load under rucc (for example "Unknown symbol" or "disagrees about version of symbol") is a failure of that suite, and 09.7's audit explains it.

`kunit.py run --arch=x86_64` (UML or QEMU) is not used, because UML (`ARCH=um`) is a separate architecture whose builds have their own flags. It is useful locally for triage, and `rk kunit-uml` wraps it ungraded.

The KUnit suites that test the compiler most directly are run first and reported separately:
- `stackinit`;
- `fortify`;
- `overflow`;
- `memcpy`;
- `string`;
- `bitfield`;
- `bits`;
- `usercopy`;
- `static_keys` (via `test_static_keys` module);
- `jump_label`;
- `kfence` (read-only);
- `siphash`;
- `crc`;
- `crypto` self tests (via `CRYPTO_MANAGER_EXTRA_TESTS`, which is in the distro config);
- `test_bpf`, which checks the JIT and the interpreter, both compiled by rucc.

## 11.5 kselftest and LTP

From K5, on Current for X64 and A64.

**kselftest** collections are pinned in `rows.toml`: `breakpoints`, `capabilities`, `clone3`, `core`, `exec`, `futex`, `kcmp`, `membarrier`, `mm`, `mqueue`, `pidfd`, `proc`, `rseq`, `seccomp`, `sigaltstack`, `size`, `timens`, `timers`, `x86` (X64 only), `arm64` (A64 only), `vDSO`, `ptrace`, `syscall_user_dispatch`, `landlock`, `splice`, `sync`, `user_events`.

They are built by `make -C tools/testing/selftests TARGETS=... CC=<rucc>`. The built programs are user programs, so rucc compiles both sides of the syscall boundary. For attribution, every collection also runs with GCC-built selftest binaries against the rucc kernel (11.7).

**LTP** 20260529 (pinned), runtest files `syscalls`, `mm`, `sched`, `ipc`, `fs`, `math`, `timers`, `pty`, `cve`, with a fixed list of excluded cases that fail under QEMU with the reference. That list lives in `exclusions.toml` with the reference's failure log. It is built by rucc and by GCC, like the selftests.

## 11.6 `rk-init`

A small C program, written for the harness, that runs as PID 1. It:
1. Mounts `proc`, `sys`, `devtmpfs`, `tmpfs`, `debugfs` and the 9p result share where the kernel has them.
2. Prints `RK-BOOTED <uname -r> <monotonic ns>`, the boot marker.
3. Reads `rk.suite` from `/proc/cmdline` and runs that suite: `smoke`, `kunit`, `kselftest:<coll>`, `ltp:<runtest>` or `bench:<name>`.
4. Writes results, syncs, and powers off with `reboot(RB_POWER_OFF)`, or `RB_HALT_SYSTEM` plus the ACPI shutdown for old kernels with `-no-reboot`.

The **smoke** suite:
- fork and exec busybox;
- `mount`;
- `dd` 64 MB through tmpfs and compare checksums;
- `sha256sum` of `/proc/kallsyms`, checked only for readability;
- 1,000 threads created and joined;
- `mmap`/`munmap` of 1 GB in 4 KB and 2 MB pages;
- a pipe round trip;
- a signal delivery;
- `ping -c 3 127.0.0.1` with loopback up;
- the virtio-blk device read end to end;
- `cat /proc/cpuinfo`, checking that all vCPUs came up;
- `rk-hog` (a CPU and memory churn for 30 s with `CONFIG_DEBUG_*` on).

About 40 checks in total, each a line in the result.

**The museum version** of `rk-init`:
- uses only syscalls that exist in 1.0: `write`, `fork`, `execve`, `waitpid`, `reboot`, `mount`, `mknod`, `open`, `read`, `close`;
- is statically linked, with no libc, using raw `int $0x80` with 1.0 syscall numbers;
- is built by rucc as i386 ELF, or a.out for 1.x (K11);
- prints the marker and runs a tiny smoke suite (fork plus exec of itself in a child mode, pipe, file write and read-back on the root filesystem);
- halts with `reboot(0xfee1dead, 672274793, 0x4321fedc)` where the kernel has it, or `hlt` via a sentinel otherwise, which the rig detects by the marker plus the absence of further output within 5 s.

## 11.7 Attribution

When a unit passes with the GCC kernel and fails with the rucc kernel, three things could be wrong:
1. the rucc-built kernel;
2. the rucc-built userland;
3. an interaction.

`rk test` runs the failing unit in a 2x2:

| | GCC userland | rucc userland |
|---|---|---|
| GCC kernel | passes (by definition, it is in G) | ? |
| rucc kernel | ? | fails |

A failure with the GCC userland is a kernel failure and goes to `rk mixed` (11.8). A failure only with the rucc userland on both kernels is a user program miscompilation. It is reported to rucc with the program and reduced through `rucc-compat` and `rucc-corpus`, not through the kernel machinery.

## 11.8 Mixed-object bisection (`rk mixed`)

This is the kernel version of `rpg mixed`. The link takes objects from both builds, so any subset of rucc objects can be combined with GCC objects, because both follow the same ABI and have the same CRCs by 06.8. The procedure:

1. List the objects that make up `vmlinux` (from `vmlinux.o`'s archive members, or `modules.order` and `built-in.a` for older kernels). There are about 3,000 for `defconfig` and 25,000 for `allmodconfig`, including modules.
2. **Delta-debug** over the set. Each trial links a kernel with the chosen subset from rucc and the rest from GCC, and runs the failing unit (a boot plus one suite, 1 to 5 minutes under KVM). With about 3,000 objects, that is about 12 trials when one object is responsible, and a few dozen in practice.
3. When a single object is found, bisect inside it by function with `-fpass-fuel-global=N` in rucc, the existing mechanism. Then diff `-fdump-ir=all` between the last good and first bad fuel.
4. Record the finding in `runs/<id>/mixed.json` with the object, the function, the pass, and a reduced C reproducer. The reproducer is reduced by `cvise` with an interestingness test that compiles and compares the function's behaviour in a user-mode harness where possible. Otherwise it is kept as a kernel-unit test.

Constraints:
- **Objects that contain `.S` code.** `rk mixed` treats rucc's assembler as a separate dimension. The GCC-side objects from `.S` are assembled by gas, and the rucc side by rucc.
- **Modules** are swapped as whole modules first, and then by object inside a module.
- **The link must be valid.** A rucc object that fails to link with GCC objects is itself a finding (ABI or section mismatch). `rk mixed` reports it and stops.

## 11.9 dmesg

Both kernels' dmesg are captured after each suite. Splats (`WARNING:`, `BUG:`, `Oops`, `general protection fault`, `KASAN:`, `UBSAN:`, `list_add corruption`, `RCU stall`, `hung task`, `soft lockup`, `objtool`-style lines, `lockdep` reports) are normalized: addresses, PIDs, timestamps and module load addresses are masked.

The set of normalized splats from the rucc kernel must be a subset of the reference's over three runs. A rucc-only splat is a failure even when every test passed. That is how the memory model failures of 06.7 are most likely to show first.
