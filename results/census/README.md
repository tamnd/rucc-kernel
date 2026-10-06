# Census results

Each directory here is one row and configuration of the census workflow, brought in by a pull request so a result never changes without a review. The gcc-14 build gives `asm-inventory.md` and `syntax.md`, the rucc build gives `demands.md`, and `probes.md` is `rk probes` over the two compile logs. The rucc each one was built with is named below.

| Directory | rucc | Units | rucc failed | Syntax failed | Probes differing |
| --- | --- | --- | --- | --- | --- |
| `7.2.8-X64-defconfig` | v0.24.8 | 3071 | 0 | 0 of 3069 | 21 of 119 |

## 7.2.8 X64 defconfig

rucc v0.24.8 compiled and assembled all 3071 units, inline assembly and `.S` files included. No unit asked for anything rucc does not have, so every mnemonic class in `asm-inventory.md`, 473 mnemonics in 1198 shapes, is one the integrated assembler already reads.

The 21 probes answered differently are all ones the plan expects:

- Six pass with both and differ only in what they print: `--version`, the assembler's version, `__LP64__`, the predefined macros, `-print-file-name=plugin` and the preprocessing `scripts/checksyscalls.sh` does, which sends its text to `/dev/null` and reads only the warnings.
- Thirteen are `-fsanitize=` modes and the coverage callbacks, which rucc refuses and plan section 8.10 keeps out of the grade. Two of them are a sanitizer with a `--param`, which rucc reads as unknown before it gets to the sanitizer.
- `-gsplit-dwarf` is refused, which plan section 9 says is a compile-only configuration and which `defconfig` does not ask for.
- `-ftrivial-auto-var-init=zero` with clang's enabler flag is a clang probe that gcc-14 passes only because it reads `-enable-...` as the linker's `-e`. Kconfig takes it or the bare flag, and both compilers pass the bare one, so the `.config` is the same.

The `kernel-pp` corpus on this build found 1024 differences, every one of them from `include/linux/ctype.h` asking `__has_builtin(__builtin_isdigit)`, which gcc-14 has and rucc v0.24.8 did not. tamnd/rucc#3102 adds it, and the corpus result goes here once the census has run against it.
