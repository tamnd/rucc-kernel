//! The command line, parsed by hand as `rpg` in rucc-postgres does it.
//!
//! A command takes at most one positional word, the kernel version, and then options. Every option
//! takes a value, given as the next word or after `=`, except the few listed as switches. A value
//! may start with a dash.

use std::collections::BTreeMap;

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// The subcommand.
    pub command: String,
    /// The positional word, which is a kernel version or pin name where a command takes one.
    pub target: Option<String>,
    /// Options with values.
    pub values: BTreeMap<String, String>,
    /// Switches that were given.
    pub switches: Vec<String>,
}

/// Options that take no value.
const SWITCHES: &[&str] = &[
    "no-upstream-check",
    "write",
    "help",
    "all",
    "keep-going",
    "twice",
    "config-only",
    "why",
    "stack-usage",
    "no-fuel",
];

/// The flags of `rk build`.
const BUILD: &[&str] = &[
    "row",
    "config",
    "cc",
    "out",
    "jobs",
    "targets",
    "bringup",
    "bringup-cc",
    "fragment",
    "keep-going",
    "twice",
    "stack-usage",
    "kcflags",
    "config-only",
    "no-upstream-check",
];

/// Options each command accepts, and whether it takes a positional word.
fn accepted(command: &str) -> Option<(bool, &'static [&'static str])> {
    Some(match command {
        "fetch" => (true, &["set", "no-upstream-check", "all"]),
        "sets" => (false, &["releases", "write"]),
        "build" => (true, BUILD),
        "config-diff" => (false, &["reference", "other", "why", "source"]),
        "probes" | "flags-diff" | "symvers-diff" | "objtool-report" => {
            (false, &["reference", "other"])
        }
        "sections-diff" => (false, &["reference", "other", "save", "all"]),
        "frames" | "btf" => (false, &["reference", "other", "save"]),
        "vec-audit" => (false, &["build", "reference", "save"]),
        "modules-audit" => (false, &["build"]),
        "syntax" => (false, &["build", "cc", "allow", "jobs"]),
        "demands" => (false, &["builds", "limit"]),
        "boot" => (
            false,
            &[
                "build",
                "row",
                "cpu",
                "initramfs",
                "busybox",
                "timeout",
                "append",
            ],
        ),
        "test" => (
            false,
            &[
                "reference",
                "other",
                "row",
                "cpu",
                "kinds",
                "busybox",
                "rucc-busybox",
                "selftests",
                "rucc-selftests",
                "ltp",
                "rucc-ltp",
                "runs",
                "timeout",
                "out",
            ],
        ),
        "selftests" => (false, &["build", "cc", "collections", "jobs", "out"]),
        "ltp" => (false, &["build", "cc", "runtests", "jobs", "out"]),
        "mixed" => (
            false,
            &[
                "reference",
                "other",
                "row",
                "cpu",
                "unit",
                "busybox",
                "selftests",
                "ltp",
                "timeout",
                "jobs",
                "out",
                "no-fuel",
            ],
        ),
        "cross-modules" => (
            false,
            &[
                "reference",
                "other",
                "row",
                "cpu",
                "busybox",
                "runs",
                "timeout",
                "out",
            ],
        ),
        "initramfs" => (false, &["out", "busybox"]),
        "asm-inventory" => (false, &["build", "jobs"]),
        "baseline" => (
            true,
            &[
                "row",
                "config",
                "cc",
                "fragment",
                "jobs",
                "runs",
                "busybox",
                "timeout",
                "stack-usage",
                "no-upstream-check",
            ],
        ),
        "personas" => (true, &["era", "engine", "for"]),
        "help" | "version" => (false, &[]),
        _ => return None,
    })
}

/// The usage text.
pub const USAGE: &str = "\
rk: build, boot and test pinned Linux kernels with rucc and with a reference compiler

usage:
  rk fetch [VERSION] [--set NAME] [--all] [--no-upstream-check]
  rk sets [--releases FILE] [--write]
  rk personas [check] [--era E9,E10,E11] [--engine docker]
  rk personas --for VERSION
  rk config-diff --reference DIR --other DIR [--why] [--source DIR]
  rk flags-diff --reference DIR --other DIR
  rk probes --reference DIR --other DIR
  rk sections-diff --reference DIR|FILE [--other DIR|FILE] [--save FILE] [--all]
  rk symvers-diff --reference DIR --other DIR
  rk vec-audit --build DIR|FILE [--reference DIR|FILE] [--save FILE]
  rk modules-audit --build DIR
  rk objtool-report --reference DIR|LOG --other DIR|LOG
  rk frames --reference DIR|FILE [--other DIR|FILE] [--save FILE]
  rk btf --reference DIR|VMLINUX|FILE [--other DIR|VMLINUX|FILE] [--save FILE]
  rk syntax --build DIR [--cc rucc] [--allow FILE] [--jobs N]
  rk boot --build DIR [--row X64] [--cpu NAME] [--initramfs FILE | --busybox PATH]
          [--timeout 300] [--append WORDS]
  rk test --reference DIR --other DIR [--kinds boot,smoke,kunit,kselftest,ltp] [--row X64]
          [--cpu NAME] [--runs 1] [--selftests DIR] [--rucc-selftests DIR] [--ltp DIR]
          [--rucc-ltp DIR] [--busybox PATH] [--rucc-busybox PATH] [--timeout 600] [--out DIR]
  rk mixed --reference DIR --other DIR --unit UNIT [--row X64] [--cpu NAME] [--timeout 600]
          [--no-fuel] [--selftests DIR] [--ltp DIR]
  rk cross-modules --reference DIR --other DIR [--row X64] [--cpu NAME] [--runs 1]
          [--timeout 600] [--busybox PATH] [--out DIR]
  rk selftests --build DIR [--cc COMPILER] [--collections timers,size] [--jobs N] [--out DIR]
  rk ltp --build DIR [--cc COMPILER] [--runtests syscalls,mm] [--jobs N] [--out DIR]
  rk initramfs --out FILE [--busybox PATH]
  rk baseline [VERSION] --cc COMPILER [--row X64] [--config defconfig] [--fragment test]
              [--runs 3] [--timeout 600] [--stack-usage]
  rk asm-inventory --build DIR [--jobs N]
  rk demands --builds \"DIR DIR ...\" [--limit 40]
  rk build [VERSION] --cc COMPILER [--row X64] [--config defconfig] [--out DIR] [--jobs N]
           [--fragment test] (--config also takes debian-13, debian-13-arm64 or fedora-44)
           [--targets \"vmlinux bzImage\"] [--keep-going] [--twice] [--stack-usage]
           [--kcflags \"-fenable-inject-fault=f\"] [--config-only]
           [--bringup m16 --bringup-cc gcc]
  rk version

The repository is found by walking up to pins.toml, or from RK_ROOT. Downloads and unpacked
trees go to RK_CACHE, or ~/.cache/rk.
";

impl Args {
    /// Parse the words after the program name.
    pub fn parse(words: &[String]) -> Result<Self, String> {
        let mut words = words.iter().peekable();
        let command = match words.next() {
            None => "help".to_string(),
            Some(w) if w == "-h" || w == "--help" => "help".to_string(),
            Some(w) if w == "--version" => "version".to_string(),
            Some(w) => w.clone(),
        };
        let (positional, allowed) =
            accepted(&command).ok_or_else(|| format!("unknown command {command}; see rk help"))?;
        let mut target = None;
        let mut values = BTreeMap::new();
        let mut switches = Vec::new();
        while let Some(word) = words.next() {
            let Some(option) = word.strip_prefix("--") else {
                if positional && target.is_none() {
                    target = Some(word.clone());
                    continue;
                }
                return Err(format!("unexpected argument {word}"));
            };
            let (name, inline) = match option.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (option, None),
            };
            if name == "help" {
                switches.push("help".to_string());
                continue;
            }
            if !allowed.contains(&name) {
                return Err(format!("rk {command} has no option --{name}"));
            }
            if SWITCHES.contains(&name) {
                if inline.is_some() {
                    return Err(format!("--{name} takes no value"));
                }
                switches.push(name.to_string());
                continue;
            }
            let value = match inline {
                Some(v) => v,
                None => words
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("--{name} needs a value"))?,
            };
            values.insert(name.to_string(), value);
        }
        Ok(Self {
            command,
            target,
            values,
            switches,
        })
    }

    /// The value of an option, if given.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Whether a switch was given.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Result<Args, String> {
        let words: Vec<String> = words.iter().map(|s| (*s).to_string()).collect();
        Args::parse(&words)
    }

    #[test]
    fn a_version_then_options() {
        let args = parse(&["fetch", "7.2.8", "--no-upstream-check"]).unwrap();
        assert_eq!(args.target.as_deref(), Some("7.2.8"));
        assert!(args.has("no-upstream-check"));
    }

    #[test]
    fn values_come_after_a_space_or_an_equals_sign() {
        let args = parse(&["fetch", "--set", "current"]).unwrap();
        assert_eq!(args.get("set"), Some("current"));
        let args = parse(&["sets", "--releases=r.json"]).unwrap();
        assert_eq!(args.get("releases"), Some("r.json"));
    }

    #[test]
    fn mistakes_are_named() {
        assert!(
            parse(&["frobnicate"])
                .unwrap_err()
                .contains("unknown command")
        );
        assert!(
            parse(&["sets", "7.2.8"])
                .unwrap_err()
                .contains("unexpected")
        );
        assert!(
            parse(&["fetch", "--row", "X64"])
                .unwrap_err()
                .contains("--row")
        );
        assert!(
            parse(&["fetch", "--set"])
                .unwrap_err()
                .contains("needs a value")
        );
    }

    #[test]
    fn nothing_at_all_is_help() {
        assert_eq!(parse(&[]).unwrap().command, "help");
    }
}
