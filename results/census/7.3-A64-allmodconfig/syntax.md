25565 units, 25554 passed, 11 failed: 2 new and 9 known.

| error | units | first unit |
|---|---|---|
| `unknown type name '_' [EN]` | 9 | `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.3/crypto/aegis128-neon-inner.c` |
| `unknown option '_'` | 2 | `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.3/arch/arm64/kernel/vdso/vgetrandom.c` |

New failures:

| unit | error |
|---|---|
| `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.3/arch/arm64/kernel/vdso/vgetrandom.c` | `rucc: error: unknown option '-finline-stringops=memset'` |
| `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.3/arch/arm64/kernel/vdso/vgettimeofday.c` | `rucc: error: unknown option '-finline-stringops=memset'` |
