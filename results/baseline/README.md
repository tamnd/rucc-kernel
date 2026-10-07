# Reference baselines

Each file here is one `rk baseline` run: `defconfig` or a pinned distro config with the test fragment, built three times with gcc-14 and booted each time under QEMU. The baseline workflow makes them and a pull request brings them in, so a baseline never changes without a review. Every build below booted all three times, and every defconfig build gave the same image all three times.

| Version | Row | Era | Units | Build (s) | Boot (s) | Image |
| --- | --- | --- | --- | --- | --- | --- |
| 7.2.8 | X64 | E11 | 3071 | 659 | kvm 2.5 | `83bae9b84dca` |
| 7.2.8 | A64 | E11 | 4406 | 549 | tcg 8.6 | `31d6a8c7659b` |
| 6.12.111 | X64 | E10 | 2953 | 442 | kvm 1.8 | `ad7ef79f9e2c` |
| 6.12.111 | A64 | E10 | 4312 | 508 | tcg 4.9 | `74a89097cf4d` |
| 6.1.188 | X64 | E10 | 2777 | 362 | kvm 1.9 | `9054fef56d65` |
| 6.1.188 | A64 | E10 | 3888 | 465 | tcg 5.1 | `e7297282c54a` |
| 7.2.8 | X32 | E11 | 2957 | 463 | kvm 1.7 | `8000f5a841c6` |
| 6.12.111 | X32 | E10 | 2874 | 587 | kvm 1.6 | `08328ab389ef` |
| 6.1.188 | X32 | E10 | 2722 | 556 | kvm 1.8 | `df4dc28b72e3` |
| 6.18.54 | A64 | E11 | 4418 | 585 | tcg 8.7 | `79316782309d` |
| 6.6.157 | A64 | E10 | 4155 | 497 | tcg 4.9 | `c014d346751b` |
| 5.15.221 | A64 | E9 | 3619 | 390 | tcg 5.0 | `98af93e152b8` |
| 5.10.270 | A64 | E8 | 3463 | 386 | tcg 5.9 | `aebcfda222df` |
| 6.12.111 | A64 debian-13-arm64 | E10 | 3233 | 618 | tcg 6.9 | differs |

Build and boot times are the mean of the three runs on a GitHub hosted runner. The A64 rows boot without KVM, so their boot times are not comparable with X64. The X32 rows are `i386_defconfig`, which is what `defconfig` gives with ARCH=i386, and they boot a static i686 busybox.

The debian-13-arm64 row is the pinned Debian 13 config, which sets `CONFIG_MODULE_SIG` with no key of its own, so each build makes a new signing key. The key lands in `.init.data` and the build ID in `.notes` follows it, which is why its three images differ. Nothing else in them does.
