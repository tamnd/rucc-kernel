25238 units, 25229 passed, 9 failed: 9 new and 0 known.

| error | units | first unit |
|---|---|---|
| `unknown type name '_' [EN]` | 9 | `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.2.8/crypto/aegis128-neon-inner.c` |

New failures:

| unit | error |
|---|---|
| `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.2.8/crypto/aegis128-neon-inner.c` | `/usr/lib/gcc/aarch64-linux-gnu/14/include/arm_neon.h:74:9: error: unknown type name '__fp16' [E0546]` |
| `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.2.8/lib/crc/crc64-neon.c` | `/usr/lib/gcc/aarch64-linux-gnu/14/include/arm_neon.h:74:9: error: unknown type name '__fp16' [E0546]` |
| `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.2.8/lib/raid/raid6/arm/recov_neon_inner.c` | `/usr/lib/gcc/aarch64-linux-gnu/14/include/arm_neon.h:74:9: error: unknown type name '__fp16' [E0546]` |
| `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.2.8/lib/raid/xor/arm64/xor-eor3.c` | `/usr/lib/gcc/aarch64-linux-gnu/14/include/arm_neon.h:74:9: error: unknown type name '__fp16' [E0546]` |
| `/home/runner/work/rucc-kernel/rk-cache/src/linux-7.2.8/lib/raid/xor/xor-neon.c` | `/usr/lib/gcc/aarch64-linux-gnu/14/include/arm_neon.h:74:9: error: unknown type name '__fp16' [E0546]` |
| `lib/raid/raid6/arm/neon1.c` | `/usr/lib/gcc/aarch64-linux-gnu/14/include/arm_neon.h:74:9: error: unknown type name '__fp16' [E0546]` |
| `lib/raid/raid6/arm/neon2.c` | `/usr/lib/gcc/aarch64-linux-gnu/14/include/arm_neon.h:74:9: error: unknown type name '__fp16' [E0546]` |
| `lib/raid/raid6/arm/neon4.c` | `/usr/lib/gcc/aarch64-linux-gnu/14/include/arm_neon.h:74:9: error: unknown type name '__fp16' [E0546]` |
| `lib/raid/raid6/arm/neon8.c` | `/usr/lib/gcc/aarch64-linux-gnu/14/include/arm_neon.h:74:9: error: unknown type name '__fp16' [E0546]` |
