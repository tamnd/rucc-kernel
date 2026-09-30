# kbuild, identity and the configuration differential

Before one line of kernel C reaches rucc's optimizer, kbuild has questioned the compiler hundreds of times. It asks what it is called, which version it is, whether it accepts each of about 150 flags, and whether a dozen small programs assemble or compile. The answers decide the `.config` and the command line of every unit. This document covers those answers, and the two differentials that check them: `rk config-diff` from 4.18 and `rk flags-diff` for everything.

## 5.1 How kbuild questions the compiler

| Mechanism | Since | Form | What rucc must do |
|---|---|---|---|
| `cc-option` | 2.6 | `$(CC) <flags> <option> -c -x c /dev/null -o <tmp>` and success means yes (older: `-S`) | fail on unknown options, which it does. Succeed on known ones without warnings: some versions treat any stderr output as no |
| `cc-option-yn`, `cc-disable-warning`, `cc-ifversion`, `cc-name`, `cc-version`, `cc-fullversion` | 2.6 to 5.x | variations on the above, `-Wno-<x>` probes, `scripts/gcc-version.sh` | `cc-disable-warning` probes `-W<x>`, and rucc must accept every `-W` GCC of the persona accepts |
| `as-option`, `as-instr` | 2.6 | `$(CC) <flags> -c -x assembler -o <tmp> -` with `printf "<insn>"` on stdin | accept `-x assembler` from stdin. The instruction probes are real tests of the assembler |
| `ld-option` | 2.6 | calls `$(LD)` | nothing for rucc |
| Kconfig `$(cc-option,...)`, `$(success,...)`, `$(shell,...)` | 4.18 | the same probes run by Kconfig, writing results into `.config` | the same, and the results are now visible in `.config` |
| `scripts/gcc-x86_64-has-stack-protector.sh`, `gcc-x86_32-has-stack-protector.sh` | 2.6.24 | compiles `int foo(void) { char X[200]; return 3; }` with `-fstack-protector` and greps the assembly for `%gs` (or `%fs` on old kernels) | the canary addressing must be what the kernel's model expects. See 5.3 |
| `scripts/gcc-goto.sh`, then `CC_HAS_ASM_GOTO` | 3.0 to 5.x | compiles a small `asm goto` | a no here turns off jump labels. On x86 from 4.17 it is fatal |
| `scripts/cc-version.sh` | 5.12 | `$(CC) -E -P -x c -` over `__clang__`, `__GNUC__`, `__GNUC_MINOR__`, `__GNUC_PATCHLEVEL__` | the persona's numbers, and no `__clang__` |
| `scripts/as-version.sh` | 5.12 | `-Wa,--version` | 4.2.2 |
| `scripts/min-tool-version.sh` | 5.13 | compares with the minimum | persona at least the minimum |
| `scripts/cc-can-link.sh` | 5.x | links a program with `$(CC)`, for `CC_CAN_LINK` and the userprogs | needs a hosted target and libc. rucc can link with the system libc on x86-64 and AArch64, and cross only with a sysroot. See 5.5 |
| `scripts/gcc-plugin.sh`, `-print-file-name=plugin` | 4.8 | looks for GCC's plugin headers | report no plugin support, in the same way the reference must. See 5.4 |
| `scripts/tools-support-relr.sh`, `scripts/pahole-version.sh`, `scripts/rustc-*` | 5.x, 6.x | linker, pahole and Rust questions | nothing for rucc |
| `CC_HAS_ASM_GOTO_OUTPUT`, `_TIED_OUTPUT`, `CC_HAS_ASM_INLINE`, `CC_HAS_NAMED_AS`, `CC_HAS_COUNTED_BY`, `CC_HAS_ASSUME`, `CC_HAS_ZERO_CALL_USED_REGS`, `CC_HAS_AUTO_VAR_INIT_*`, `CC_HAS_SANE_FUNCTION_ALIGNMENT`, `CC_HAS_KASAN_*`, `CC_HAS_UBSAN_*`, `CC_HAS_KCOV`, `CC_HAS_IBT`, `CC_HAS_RETURN_THUNK`, `CC_HAS_ENTRY_PADDING`, `CC_HAS_SLS` and more | 5.x to 7.x | `$(success,echo '<program>' \| $(CC) -x c - -c -o /dev/null <flags>)` or version tests | each is a real feature question, graded by 5.2 |

## 5.2 The configuration differential (`rk config-diff`)

From 4.18 on, compiler questions leave their answers in `.config`. The procedure:

1. Unpack V twice, into `ref/` and `rucc/`.
2. Run `make ARCH=<a> <config target>` in each, with `CC=<reference gcc>` and with `CC=<rucc persona>`. Both use the reference's binutils, `HOSTCC` and container.
3. Merge the test fragment and run `olddefconfig` in each.
4. Parse both `.config` files, remove the lines listed in `config-divergences.toml`, and compare.

Any difference is a failure and blocks the build. It is reported as a table of the options that differ, and for each option the Kconfig expression that decided it, found by `rk config-diff --why`. `--why` reads the Kconfig `depends on` and `def_bool` for the option and reruns the probe the option names, with both compilers, showing the command and both exit statuses. The failure is then usually one line away from its cause, a flag rucc lacks.

Most `CC_HAS_*` probes compile a two-line program with one flag. Several are version tests rather than probes, and those follow from the persona.

`config-divergences.toml`, as it stands at K0:

```toml
[[allowed]]
option = "CONFIG_CC_VERSION_TEXT"
why = "the banner names rucc; no Kconfig symbol depends on its text"

[[allowed]]
option = "CONFIG_GCC_VERSION"
why = "equal by persona (4.2 rule 2); listed because the second reference (GCC 16) differs"
only-against = "second-reference"
```

An entry must name the option, why it cannot change which code is compiled, and the version range. A new entry needs a review in `rucc-kernel`. An entry that hides a difference in generated code is a bug in the harness.

**Transitive options.** Some differences are consequences of others. For example, `CC_HAS_NAMED_AS=n` pulls `USE_X86_SEG_SUPPORT` with it. `rk config-diff` reports every one, but marks the roots, the options whose own probes differ, so the report leads with the few causes and not with dozens of consequences.

## 5.3 Probes that test code generation

A few probes do more than check that a flag parses. They inspect generated code, so they fail even when the flag exists.

| Probe | What it checks | rucc risk |
|---|---|---|
| `gcc-x86_64-has-stack-protector.sh` | `-mcmodel=kernel -fno-PIE -fstack-protector -S` output contains `%gs:` | rucc currently puts the canary at `%fs:40`. With `-mstack-protector-guard-reg=gs`, and in 6.x `-mstack-protector-guard-symbol=__ref_stack_chk_guard`, it must switch |
| `gcc-x86_32-has-stack-protector.sh` | with `-m32 -mstack-protector-guard-reg=fs`, the output uses `%fs:__stack_chk_guard` | needs i686 |
| the `asm goto` probes | the probe compiles; some versions test tied outputs and that a label is reached | #221 |
| `CC_HAS_SANE_STACKPROTECTOR` | the above run by Kconfig | if no, `STACKPROTECTOR` quietly turns off, and 5.2 catches it |
| `CC_HAS_RETURN_THUNK` | `-mfunction-return=thunk-extern` is accepted | the flag must also do its job; objtool validates it |
| `CC_NO_STACKPROTECTOR`, `-fno-stack-protector` probes | the flag parses | trivial |
| `arch/x86/Makefile` `-mindirect-branch=thunk-extern` probe | parses | the check that it does its job is objtool's |
| arm64 `CC_HAVE_SHADOW_CALL_STACK`, `CC_HAS_BRANCH_PROT_PAC_RET_BTI`, `CC_HAS_SIGN_RETURN_ADDRESS` | flag probes | K6 |

The table in 5.1, turned into a checklist per version, is `rk probes V`. It runs every probe kbuild will run for version V against both compilers, without building anything. It is the fastest signal the harness has: about 5 seconds per version, against 10 minutes to build. It is also the first thing K0 automates.

## 5.4 GCC plugins

From 4.8, `GCC_PLUGINS` is available when `$(CC) -print-file-name=plugin` names a directory containing `include/plugin-version.h`. The distribution GCC often has the plugin development package. rucc has no plugins.

- The reference build runs in a container **without** `gcc-<v>-plugin-dev`. That makes `GCC_PLUGINS=n` on both sides by construction.
- `rk personas check` verifies this.
- `-print-file-name=plugin` from rucc returns the bare word `plugin`, as GCC does for a missing file.

This is a choice about the reference, not a patch. The configuration a distribution builds with plugins (STRUCTLEAK, STACKLEAK, LATENT_ENTROPY, RANDSTRUCT) is not claimed, because no compiler other than GCC can build it. `RANDSTRUCT` in 5.19 and later can use Clang's `-frandomize-layout-seed`, and rucc could implement that one day. It is not in this plan.

## 5.5 Host tools and `HOSTCC`

The kernel builds programs for the build host: `fixdep`, `modpost`, `genksyms`, `conf`, `objtool`, `resolve_btfids`, `sorttable`, `insn_decoder_test` and others. Graded builds use the era container's GCC as `HOSTCC`. `HOSTCC=rucc` is K12. It is a separate, useful test, since `objtool` is a sizeable C program with `libelf`, but it confuses the diagnosis. When a boot fails, one wants to know whether the kernel or `modpost` was miscompiled.

`CC_CAN_LINK` and userprogs (samples, `usr/include` checks, `tools/testing/selftests`) are built with `$(CC)` for the target. On cross builds that needs a target sysroot. `rucc-kernel` uses the era container's cross libc (`libc6-dev-arm64-cross` and friends) for the reference. rucc uses the same sysroot through `--sysroot`, passed in `CC`, which 2.1 permits because it names where the target's headers are. It does not change code generation.

The selftests themselves are built with rucc as well, at K5, because they are target programs. That makes kselftest a test of rucc twice over, and document 11 explains how failures are attributed.

## 5.6 The flags differential (`rk flags-diff`)

Before 4.18 Kconfig did not record compiler answers. The answers live in the command line of each unit instead. `rk flags-diff` compares those command lines, and on 4.18 and later it runs as well as `config-diff`, catching `cc-option` decisions that never reach Kconfig.

1. Build with the reference and with rucc, both with `V=1`, or read the `.<obj>.cmd` files kbuild writes beside every object. That is better, because it is structured and available since 2.6.
2. For each object, extract the compiler command. Replace the compiler path with `CC`, strip `-Wp,-MD,<file>` and the output name, and sort nothing, because order matters for `-f` pairs.
3. Compare per object. A difference is a failure unless it matches a rule in `config-divergences.toml`'s `[[flags]]` table. The table starts empty.

In older kernels most `cc-option` flags are optimizations or warnings. A missing `-fno-delete-null-pointer-checks` or `-fno-strict-overflow` changes semantics. A missing `-mno-sse` would be a disaster, but rucc refuses such flags today, and a refused flag in `cc-option` means "not supported", so kbuild silently drops it. This is the single most important reason for `flags-diff`. **`cc-option` turns every rucc refusal into a silent change of the kernel's flags.** Examples with severe consequences:
- `-fno-PIE`;
- `-mno-sse`, `-mno-mmx`, `-mno-80387`;
- `-mpreferred-stack-boundary=3`;
- `-mno-red-zone` on kernels that probe it;
- `-fno-stack-protector`;
- `-fconserve-stack`;
- `-fno-delete-null-pointer-checks`.

`flags-diff` is therefore not optional for any era. It gates every build from K1.

## 5.7 What rucc must add for kbuild (K0 and K1 work list)

Driver and identity, in rucc:

1. The persona banner of 4.2.1, `-dumpfullversion`, `-print-file-name=plugin`.
2. The persona's default dialect: gnu89 for a persona below 5, gnu11 for 5 to 14, gnu17 for 15, gnu23 from 15 as GCC does. Plus `-fcommon` by default for personas below 10.
3. `-Wa,` handling:
   - accept `--version` (4.2.2), `--noexecstack`, `--fatal-warnings`, `-mx86-used-note=no`, `-gdwarf-*`, `--gdwarf-*`, `-march=*`, `-mabi=*`, `-mrelax`, `-mno-relax`, `-I<dir>`, `--64`, `--32`;
   - honor the ones that mean something;
   - refuse anything else, naming the option.

   The same list applies to `-Xassembler`.
4. `-fgnu-as-version=`, the assembler persona value.
5. The no-op and one-line flags of document 03.2, each with its GCC meaning written in the option table. Where rucc cannot honor the meaning, the flag is refused, and then `flags-diff` fails. The rule is that rucc never pretends.
6. `-W<x>` and `-Wno-<x>`: accept every warning name GCC 16 knows, as no-ops, so `cc-disable-warning` behaves as it does under GCC. The list comes from `gcc --help=warnings`, pinned in `rucc/crates/rucc-driver/data/gcc-warnings.txt`. Unknown `-W` names must fail as GCC's do, or a `cc-option` probe of a misspelt warning would disagree.
7. Target from program name: `aarch64-linux-gnu-gcc`, `x86_64-linux-gnu-gcc` and `i686-linux-gnu-gcc`, as symlinks to rucc, imply `--target`.
8. `__ASSEMBLER__` defined for `.S` input, macro expansion of `$SYM`, `#` comment lines in `.S`. Document 07.1.

In `rucc-kernel`:
- `rk probes`, `rk config-diff`, `rk flags-diff`;
- `config-divergences.toml`.

Exit for this document's part of K1: `rk probes` and `rk config-diff` are clean for x86-64 `defconfig` on 7.2.x, 6.12.y and 5.15.y, and `rk flags-diff` is clean on those three plus 4.19.y and 4.4.y.
