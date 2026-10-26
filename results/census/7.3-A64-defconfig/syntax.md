4433 units, 4430 passed, 3 failed: 2 new and 1 known.

| error | units | first unit |
|---|---|---|
| `unknown option '_'` | 2 | `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.3/arch/arm64/kernel/vdso/vgetrandom.c` |
| `unknown type name '_' [EN]` | 1 | `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.3/lib/crc/crc64-neon.c` |

New failures:

| unit | error |
|---|---|
| `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.3/arch/arm64/kernel/vdso/vgetrandom.c` | `rucc: error: unknown option '-finline-stringops=memset'` |
| `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.3/arch/arm64/kernel/vdso/vgettimeofday.c` | `rucc: error: unknown option '-finline-stringops=memset'` |
