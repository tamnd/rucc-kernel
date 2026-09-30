# Language, builtins and optimization as correctness

The kernel is written in GNU C, and its idioms lean on corners that ordinary programs never touch. This document lists the idioms by era, with rucc's status, and then deals with the kernel's defining peculiarity: some of its correctness depends on the optimizer.

## 6.1 The dialect by era

| Era | Standard | Notable GNU dependencies |
|---|---|---|
| 1.x, 2.0 | K&R and C89 by default (GCC 2.x) | `extern inline` with GNU89 semantics used as a macro. `__asm__` with register bindings everywhere. Cast as lvalue (`(char *)p += n`). Multi-line string literals in inline asm. `__attribute__` rarely, `__label__` never. `-traditional` preprocessing of `.S` in some Makefiles. `.byte` sequences for missing instructions |
| 2.2, 2.4 | gnu89 | as above, plus conditional as lvalue, `__builtin_constant_p` in `string.h`, labels at the end of compound statements, zero length arrays, `typeof`, statement expressions |
| 2.6.x | gnu89 | `__attribute__((section))` everywhere (initcalls, `__init`), `__builtin_expect`, `__builtin_constant_p` on `size` in `copy_*_user`, `__attribute__((regparm(3)))` on i386, `asmlinkage`, `fastcall` |
| 3.x to 5.17 | `-std=gnu89` from 3.18 (default gnu89 before) | `__builtin_choose_expr`, `__builtin_types_compatible_p`, `asm goto` (3.0 on), `__same_type`, `__is_constexpr` (4.17 on), `__compiletime_assert` with `__attribute__((error))`, `__builtin_*_overflow` (4.18 on), `__counted_by`-less flexible arrays |
| 5.18 on | `-std=gnu11` | declarations in `for`, `_Static_assert`, `_Generic` in `READ_ONCE`, `__auto_type` in `min`/`max`, `__typeof_unqual__` (6.x), `__builtin_dynamic_object_size`, `counted_by`, `__builtin_counted_by_ref` (6.15), `-fms-extensions` tagged anonymous members (6.19), `-fms-anonymous-structs` (7.1), `const` variables in integer constant expressions (7.2), `__attribute__((cleanup))` for `guard()` and `scoped_guard()` (6.5), `__builtin_assume` via `assume` attribute (6.x), `__attribute__((__counted_by__))` |

What rucc lacks is listed in document 03.3. The rest of this document works through the idioms that are hard for reasons other than a missing feature.

## 6.2 `__is_constexpr` and null pointer constants

```c
#define __is_constexpr(x) \
	(sizeof(int) == sizeof(*(8 ? ((void *)((long)(x) * 0l)) : (int *)8)))
```

If `x` is an integer constant expression, `(void *)((long)(x) * 0l)` is a null pointer constant. Then the conditional has type `int *`, and `sizeof(*...)` is `sizeof(int)`. Otherwise it is a `void *` expression, the result is `void *`, and `sizeof(void)` is 1 in GNU C. rucc recognizes only a literal `0` as a null pointer constant (`crates/rucc-sema/src/convert.rs:250`), so the macro answers "not constant" for everything.

The effect is not a compile error. `min()` and `max()` in 5.x to 6.1 choose between a statement expression and a plain conditional based on this answer. Array declarations whose size uses `max()` then become variable length arrays, which rucc accepts, and which the kernel forbids with `-Wvla`. That is harmless in behaviour but changes stack layout, and some `BUILD_BUG_ON_ZERO(!__is_constexpr(x))` uses stop compiling.

The fix: an integer constant expression of value 0, cast to `void *` either directly or through an integer type, is a null pointer constant (C11 6.3.2.3). **S, K1.** It has a rucc-corpus facet of its own.

## 6.3 `const` in integer constant expressions

7.2 raised the minimum Clang to 17.0.1 because older Clang does not fold `const` locals into integer constant expressions the way GCC does. Commit `ce3267a39a92` cites:

```c
const int n = 4;
_Static_assert(n == 4, "");   // GCC: accepted as an extension; rucc: E0614
char buf[n];                   // GCC: not a VLA; rucc: VLA
```

GCC treats a `const` object of integer type, with a constant initializer and not `volatile`, as foldable in these contexts, and warns only under `-Wpedantic`. rucc must do the same, under GNU dialects only. **S to M, K1.**

## 6.4 `__builtin_constant_p` and the optimization dependency

The kernel's defining trick:

```c
static __always_inline void *kmalloc(size_t size, gfp_t flags)
{
	if (__builtin_constant_p(size) && size) {
		if (size > KMALLOC_MAX_CACHE_SIZE)
			return kmalloc_large(size, flags);
		/* index computed at compile time */
		...
	}
	return __kmalloc(size, flags);
}
```

This is well defined at any level. The dangerous form is the one where the untaken branch **must not survive**, because it calls a function that does not exist:

```c
extern void __bad_size_call_parameter(void);   /* never defined */

#define __pcpu_size_call_return(stem, var)		\
({	switch (sizeof(var)) {				\
	case 1: ... case 2: ... case 4: ... case 8: ...	\
	default: __bad_size_call_parameter(); break;	\
	} })
```

Similar forms:
- `__compiletime_assert(cond, msg)` declares `extern void __compiletime_assert_N(void) __attribute__((error(msg)))`. It calls that function under `if (!(cond))`, where `cond` is constant only after inlining.
- `BUILD_BUG_ON(!__builtin_constant_p(x))` inside an `__always_inline` helper.
- The FORTIFY checks in `fortify-string.h`, which call `__read_overflow()` and friends, declared `__attribute__((error))`.

These rely on three things:
1. **`__builtin_constant_p` is answered after inlining.** GCC resolves it late, and a call in a helper sees the caller's constant.
2. **Dead branches are removed after that answer.** This must happen at `-O2` and `-Os`, in every function, with no size or complexity limit.
3. **No transformation keeps a call alive** through jump threading, tail duplication, or a switch lowered into a table that still references the default case.

rucc answers `__builtin_constant_p` late (#1768, #1849), but our probe shows a case where the dead call survives. The dependency must become a stated contract in rucc's spec, under a rule like this:

> Under `-O1` and above, after inlining of `always_inline` functions, a `__builtin_constant_p` whose argument has become constant is folded to 1, and one whose argument is not constant is folded to 0 no later than the last scalar cleanup. Every branch made unreachable by either folding is removed before code generation, and no reference to a symbol appears in the object from code that is unreachable after that folding.

**M, K2.** Exit check: the rucc-corpus facet `constant-p-after-inline`, which has 50 shapes drawn from `percpu.h`, `fortify-string.h`, `slab.h`, `bitops.h`, `uaccess.h` and `compiler_types.h`. Each shape must produce an object with no undefined reference to a `__bad_*`, `__compiletime_assert_*`, `__read_overflow*` or `__write_overflow*` symbol, at `-O2` and `-Os`.

`__attribute__((error))` must also be honored (document 03.3). Then a real failure is a compile error naming the kernel's message, and not a link error naming `__compiletime_assert_417`. That makes triage minutes instead of hours.

## 6.5 Other optimization dependencies

| Dependency | Where | rucc must |
|---|---|---|
| `switch (sizeof(x))` in `__put_user_size` and `__get_user_size` with an undefined `default` | uaccess, pre 5.x | fold `sizeof` switches, no table |
| `if (0) { undefined(); }` from config macros, such as `IS_ENABLED(CONFIG_FOO) && foo()` with `foo` defined only when `CONFIG_FOO` | everywhere, and relied on even at `-O1` | remove constant false branches at every level we grade (always the case in rucc at `-O1`+, pin it with a test) |
| `static inline` functions never called but referencing undefined symbols | everywhere | never emit an unused `static inline` |
| `static` variables used only in dead code must vanish | `DEFINE_STATIC_KEY_*` in `#ifdef`-light code | not emitted, or the section counts in 9.4 differ |
| `__builtin_unreachable()` after a `BUG()` asm | `BUG()`, `unreachable()` | end the block, no fall-through code, and objtool must see the `ud2` as the end. Document 09 |
| no calls to `memcpy`, `memset` or `memmove` invented in `noinstr` text, or in `__init` code before the alternatives run on some arches | x86 `noinstr`, early boot | expand small constant-size copies inline, and never call the library for a struct copy under `-ffreestanding` in `noinstr` sections |
| calls to `__builtin_memcpy` with a constant small size inline | performance and `noinstr` | inline up to 64 bytes on x86-64 with general-purpose registers only |
| stack frames bounded: `-Wframe-larger-than=2048` (1024 on 32-bit) | `CONFIG_FRAME_WARN` | measured, not warned. Document 08.8 |
| inlining of `always_inline` into `noinstr` and `__init` functions | x86 entry code | must work across sections |

## 6.6 Semantics the kernel assumes

| Assumption | Flag that states it | rucc status |
|---|---|---|
| signed overflow wraps | `-fno-strict-overflow`, and `-fwrapv` in some trees | accepted. Must be verified as honored in every pass: a facet in rucc-corpus |
| no type based alias analysis | `-fno-strict-aliasing` | honored |
| null checks are kept after a dereference | `-fno-delete-null-pointer-checks` | check. Must be honored. An S issue if it is only parsed |
| no store data races are invented | `-fno-allow-store-data-races` (GCC 10+), `--param=allow-store-data-races=0` before | unknown flag today. The semantics are 6.7 |
| enum sizes as `int` | none | yes |
| `char` signedness per arch, and `-funsigned-char` from 6.2 everywhere | `-funsigned-char` | honored |
| `long` is pointer sized, with `__int128` on 64-bit | none | yes |
| bitfield layout matches GCC's, including `packed` and `aligned` on members, and zero width fields | none | the layout must be byte-identical. That is covered by the rucc-compat ABI corpus for user programs. The kernel adds UAPI structs checked by `BUILD_BUG_ON` and by `pahole` against DWARF |
| `volatile` accesses are single, full-width instructions | `READ_ONCE`, `WRITE_ONCE` | 6.7 |

## 6.7 The kernel memory model

The Linux kernel memory model (`tools/memory-model`) is defined in terms of what the compiler does with plain, `volatile` and atomic accesses, and it assumes things C11 does not promise:

1. **`volatile` accesses of machine-word size or smaller are single instructions.** No tearing, no splitting, no merging, no elimination, no invention. `READ_ONCE` and `WRITE_ONCE` rely on this, and so does most of RCU.
2. **No invented stores.** A conditional store must not become an unconditional store plus a restore. Nor may a store to one field of a struct become a wider store over neighbours. GCC's `-fno-allow-store-data-races` enforces the first for the optimizer.
3. **Control dependencies are kept.** In

   ```c
   q = READ_ONCE(a);
   if (q)
       WRITE_ONCE(b, 1);
   ```

   the store must not be hoisted above the branch. Nor may the branch be turned into a conditional move, or merged with an identical store in the else arm. `memory-barriers.txt` documents what compilers may and may not do here.
4. **Address and data dependencies are kept** through `rcu_dereference`. The compiler must not replace a pointer loaded with `READ_ONCE` by an equal pointer it knows about. Value speculation breaks the dependency.
5. **Plain accesses may tear**, and the kernel tolerates that. Where it cannot, it uses `data_race()` or the `*_ONCE` macros.

rucc has never been tested against these. The plan:
- a new rucc-corpus facet `lkmm` with one test program per rule above, drawn from `memory-barriers.txt` and the litmus tests' C form;
- each program is compiled at `-O2` and `-Os`, and its assembly checked by a pattern file;
- the check is structural, "exactly one store to `b`, after a branch on the load of `a`", written as FileCheck-style directives that `rucc-corpus` already supports.

**M, K3.** A failure here is the kind of miscompilation that boots fine and corrupts memory under load a month later. It is the first thing to suspect when KUnit passes and LTP stress fails.

## 6.8 The kernel `-E` differential (`kernel-pp`)

The first check on any kernel is whether rucc's preprocessor produces what GCC's does, because every kbuild probe and every generated header runs through it. `rucc-compat` gains a corpus called `kernel-pp`:
- For each graded version, the `-E` output of about 200 representative units, with GCC's persona-matching reference and with rucc, in `defconfig`. The units are chosen by `rk demands` to cover every directory's headers.
- The outputs are compared after normalizing line markers and whitespace. Differences are failures unless listed as a known, harmless difference, such as the `__VERSION__` string or `__DATE__`.

Also:
- The x86 and arm64 `asm-offsets.c` outputs, which are compiled to `.s` and scanned by `sed` for `->` markers. Those are compared as text too. rucc's `.s` must carry the markers in the same form, or `offsets.h` comes out empty.
- `genksyms` reads `-E` output to compute symbol CRCs for `MODVERSIONS`. The CRC of every exported symbol must be equal between the two builds, or modules built by GCC will not load into rucc's kernel. This is the real check, and K5's cross-modules test depends on it. It is covered at K1 by comparing `-E` output for every file that exports a symbol.

**M for the harness, K0.** The rucc fixes it finds are mostly S.
