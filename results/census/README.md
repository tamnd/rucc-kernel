# Census results

Each directory here is one row and configuration of the census workflow, brought in by a pull request so a result never changes without a review. The gcc-14 build gives `asm-inventory.md` and `syntax.md`, the rucc build gives `demands.md`, and `probes.md` is `rk probes` over the two compile logs. The rucc each one was built with is named below.

| Directory | rucc | Units | rucc failed | Syntax failed | Probes differing |
| --- | --- | --- | --- | --- | --- |
| `7.2.8-X64-defconfig` | v0.24.8 | 3071 | 0 | 0 of 3069 | 21 of 119 |
| `7.2.8-A64-defconfig` | v0.30.12 | 4406 | 0 | 1 of 4396, known | 16 of 119 |
| `7.2.8-A64-allmodconfig` | v0.30.12 | 25253 | 0 | 9 of 25238, known | 970 of 2074 |

## 7.2.8 X64 defconfig

rucc v0.24.8 compiled and assembled all 3071 units, inline assembly and `.S` files included. No unit asked for anything rucc does not have, so every mnemonic class in `asm-inventory.md`, 473 mnemonics in 1198 shapes, is one the integrated assembler already reads.

The 21 probes answered differently are all ones the plan expects:

- Six pass with both and differ only in what they print: `--version`, the assembler's version, `__LP64__`, the predefined macros, `-print-file-name=plugin` and the preprocessing `scripts/checksyscalls.sh` does, which sends its text to `/dev/null` and reads only the warnings.
- Thirteen are `-fsanitize=` modes and the coverage callbacks, which rucc refuses and plan section 8.10 keeps out of the grade. Two of them are a sanitizer with a `--param`, which rucc reads as unknown before it gets to the sanitizer.
- `-gsplit-dwarf` is refused, which plan section 9 says is a compile-only configuration and which `defconfig` does not ask for.
- `-ftrivial-auto-var-init=zero` with clang's enabler flag is a clang probe that gcc-14 passes only because it reads `-enable-...` as the linker's `-e`. Kconfig takes it or the bare flag, and both compilers pass the bare one, so the `.config` is the same.

The `kernel-pp` corpus on this build found 1024 differences, every one of them from `include/linux/ctype.h` asking `__has_builtin(__builtin_isdigit)`, which gcc-14 has and rucc v0.24.8 did not. tamnd/rucc#3102 adds it, and the corpus result goes here once the census has run against it.

## 7.2.8 A64 defconfig and allmodconfig

From census runs 40199218825 (defconfig) and 40193235543 (allmodconfig), with rucc at tamnd/rucc@de92c88, which reports itself as v0.30.12. rucc compiled every unit in both, and no unit asked for anything rucc does not have. The integrated assembler read every unit's inline assembly, 410 mnemonics for defconfig and 518 for allmodconfig, so no mnemonic class is missing.

The units that fail the syntax replay are the ones that include `<arm_neon.h>`. The kernel adds gcc's include directory with `-isystem`, so the replay reads gcc's header, which needs `__fp16`, the `#pragma GCC aarch64 "arm_neon.h"` types and the `__builtin_aarch64_*` builtins. rucc's own build reads rucc's header and compiles them. They are listed in `syntax-known.toml` under tamnd/rucc#3861.

The 16 defconfig probes answered differently:

- Five pass with both and differ only in what they print: `--version`, the assembler's version, `-print-file-name=include`, `-print-file-name=plugin` and the preprocessing `scripts/checksyscalls.sh` does.
- Eight are `-fsanitize=` modes, the shadow call stack among them, which rucc refuses and plan section 8.10 keeps out of the grade. Two of them come with a `--param` that rucc reads as unknown first.
- `-gsplit-dwarf` is refused, which hides `DEBUG_INFO_SPLIT`. It is off in the reference too.
- `-ftrivial-auto-var-init=zero` with clang's enabler flag is the same clang probe as on X64.
- `.arch_extension lsui` is taken by rucc's assembler and not by the runner's binutils 2.42, which turns on `ARM64_LSUI` in the rucc build. `config-divergences.toml` and `sections-divergences.toml` explain both.

allmodconfig has the same 16 and three more groups:

- 920 are the uapi header test compiling each header in `usr/include` as C++. rucc is a C compiler and says no to `-c -x c++ /dev/null`, so the kernel skips these checks in the rucc build.
- 33 are cc-option probes run with the KASAN flags on the command line. allmodconfig builds generic KASAN with gcc and the hardware tag mode with rucc, so the two builds ask with different command lines and each side has probes the other does not.
- 1 is the `-c -x c++ /dev/null` probe itself.
