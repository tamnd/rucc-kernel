//! `rk syntax`: every unit of a reference build through another compiler's front end.
//!
//! This is K1's check that rucc reads the whole kernel before it has to compile any of it. Each
//! unit the reference compiled is run again with the command line from `compile.jsonl`, in the
//! same directory and with the same environment, with the output and dependency file flags taken
//! out. A C unit gets `-fsyntax-only` and an assembly unit `-E`, since what K1 asks of `.S` files
//! is that they preprocess. When the compiler is rucc it gets the era's persona from
//! `build.json`, as a build would give it.
//!
//! Failures are grouped by their first error, and a file of known failures, each with an issue,
//! keeps the exit status for new ones. The replay needs the reference's output directory where it
//! was built, since units include its generated headers.

use crate::build::{error_key, is_unit, unit_source};
use rk_shim::record::CompileRecord;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long one unit may take before it counts as a failure. rucc reads the biggest kernel unit
/// in a few seconds, so this only catches a hang.
const TIMEOUT: Duration = Duration::from_secs(300);

/// The arguments to replay a unit with: the reference's, without the compiler, the output, the
/// dependency file flags, `-c` and any persona, followed by `-fsyntax-only` for C or `-E` for
/// assembly, and then `extra`.
#[must_use]
pub fn replay_args(record: &CompileRecord, extra: &[String]) -> Vec<String> {
    let assembly = record
        .inputs
        .iter()
        .any(|i| Path::new(&i.path).extension().is_some_and(|e| e == "S"));
    let mut out = Vec::new();
    let mut args = record.argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "-o" | "-MF" | "-MT" | "-MQ") {
            args.next();
            continue;
        }
        if matches!(arg.as_str(), "-c" | "-MD" | "-MMD" | "-S" | "-E")
            || (arg.starts_with("-o") && arg.len() > 2)
            || arg.starts_with("-Wp,-MD,")
            || arg.starts_with("-Wp,-MMD,")
            || arg.starts_with("-fgnuc-version=")
            || arg.starts_with("-fgnu-as-version=")
        {
            continue;
        }
        out.push(arg.clone());
    }
    out.push(if assembly { "-E" } else { "-fsyntax-only" }.to_string());
    out.extend(extra.iter().cloned());
    out
}

/// How one unit went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replayed {
    /// The source, relative to the tree.
    pub unit: String,
    /// The first error, normalized, or `None` when it passed.
    pub error: Option<String>,
    /// The first line that says error, as the compiler wrote it.
    pub line: String,
}

/// Run a compiler, with a time limit, and say how it went.
fn replay(compiler: &Path, record: &CompileRecord, args: &[String], unit: String) -> Replayed {
    let child = Command::new(compiler)
        .args(args)
        .current_dir(&record.cwd)
        .envs(&record.env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            return Replayed {
                unit,
                error: Some("could not run the compiler".to_string()),
                line: e.to_string(),
            };
        }
    };
    let mut pipe = child.stderr.take().expect("stderr was piped");
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut pipe, &mut text);
        text
    });
    let clock = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if clock.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let stderr = reader.join().unwrap_or_default();
    let line = stderr
        .lines()
        .find(|l| l.contains("error"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let error = match status {
        Some(s) if s.success() => None,
        Some(s) => Some(error_key(&stderr).unwrap_or_else(|| match s.code() {
            Some(code) => format!("exit {code} with no error message"),
            None => "killed by a signal".to_string(),
        })),
        None => Some(format!("no answer in {} seconds", TIMEOUT.as_secs())),
    };
    Replayed { unit, error, line }
}

/// Replay every unit the reference compiled, `jobs` at a time, in unit order.
#[must_use]
pub fn run(
    records: &[CompileRecord],
    tree: &Path,
    compiler: &Path,
    extra: &[String],
    jobs: usize,
) -> Vec<Replayed> {
    let work: Vec<&CompileRecord> = records
        .iter()
        .filter(|r| is_unit(r) && r.succeeded() && r.delegated.is_none())
        .collect();
    let next = Mutex::new(work.into_iter());
    let done = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs.max(1) {
            scope.spawn(|| {
                while let Some(record) = next.lock().ok().and_then(|mut it| it.next()) {
                    let args = replay_args(record, extra);
                    let result = replay(compiler, record, &args, unit_source(record, tree));
                    match done.lock() {
                        Ok(mut d) => d.push(result),
                        Err(poisoned) => poisoned.into_inner().push(result),
                    }
                }
            });
        }
    });
    let mut done = done
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    done.sort_by(|a, b| a.unit.cmp(&b.unit));
    done.dedup_by(|a, b| a.unit == b.unit && a.error == b.error);
    done
}

/// The file of known failures.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Known {
    /// Every known failure.
    #[serde(default, rename = "known")]
    pub entries: Vec<KnownFailure>,
}

/// A unit that is known to fail, and the issue that fixes it.
#[derive(Debug, Clone, Deserialize)]
pub struct KnownFailure {
    /// The source relative to the tree. A trailing `*` matches every unit with that prefix.
    pub unit: String,
    /// The issue, as `tamnd/rucc#123`.
    pub issue: String,
}

impl Known {
    /// Read the file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Read the text of the file. Every entry needs an issue.
    pub fn parse(text: &str) -> Result<Self, String> {
        let known: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if let Some(k) = known.entries.iter().find(|k| k.issue.trim().is_empty()) {
            return Err(format!("{} has no issue", k.unit));
        }
        Ok(known)
    }

    /// The issue for a unit, if it is known to fail.
    #[must_use]
    pub fn issue(&self, unit: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|k| match k.unit.strip_suffix('*') {
                Some(prefix) => unit.starts_with(prefix),
                None => k.unit == unit,
            })
            .map(|k| k.issue.as_str())
    }
}

/// The run, sorted against the known failures.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    /// Units replayed.
    pub units: usize,
    /// Failures by normalized error: how many and the first unit.
    pub errors: Vec<(String, usize, String)>,
    /// Failures no known entry covers, with the compiler's error line.
    pub new: Vec<(String, String)>,
    /// Failures a known entry covers, with its issue.
    pub known: Vec<(String, String)>,
    /// Known entries naming a unit that now passes.
    pub fixed: Vec<(String, String)>,
}

impl Summary {
    /// Sort a run.
    #[must_use]
    pub fn new(results: &[Replayed], known: &Known) -> Self {
        let mut summary = Self {
            units: results.len(),
            ..Self::default()
        };
        let mut errors: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
        for r in results {
            match (&r.error, known.issue(&r.unit)) {
                (Some(e), issue) => {
                    let entry = errors.entry(e).or_insert((0, &r.unit));
                    entry.0 += 1;
                    match issue {
                        Some(i) => summary.known.push((r.unit.clone(), i.to_string())),
                        None => summary.new.push((r.unit.clone(), r.line.clone())),
                    }
                }
                (None, Some(i)) if named_exactly(known, &r.unit) => {
                    summary.fixed.push((r.unit.clone(), i.to_string()));
                }
                (None, _) => {}
            }
        }
        summary.errors = errors
            .into_iter()
            .map(|(e, (n, unit))| (e.to_string(), n, unit.to_string()))
            .collect();
        summary
            .errors
            .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        summary
    }

    /// Whether every failure is known.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.new.is_empty()
    }

    /// The run as markdown.
    #[must_use]
    pub fn report(&self) -> String {
        let failed = self.new.len() + self.known.len();
        let mut s = format!(
            "{} units, {} passed, {failed} failed: {} new and {} known.\n",
            self.units,
            self.units - failed,
            self.new.len(),
            self.known.len()
        );
        if !self.errors.is_empty() {
            s.push_str("\n| error | units | first unit |\n|---|---|---|\n");
            for (error, n, unit) in &self.errors {
                let _ = writeln!(s, "| {} | {n} | `{unit}` |", cell(error));
            }
        }
        if !self.new.is_empty() {
            s.push_str("\nNew failures:\n\n| unit | error |\n|---|---|\n");
            for (unit, line) in self.new.iter().take(100) {
                let _ = writeln!(s, "| `{unit}` | {} |", cell(line));
            }
            if self.new.len() > 100 {
                let _ = writeln!(s, "\nand {} more.", self.new.len() - 100);
            }
        }
        if !self.fixed.is_empty() {
            s.push_str("\nKnown failures that now pass, which can come off the list:\n\n");
            for (unit, issue) in &self.fixed {
                let _ = writeln!(s, "- `{unit}` ({issue})");
            }
        }
        s
    }
}

/// Whether an entry names the unit itself. A unit known only through a prefix entry is not
/// reported as fixed when it passes, since the entry still covers others.
fn named_exactly(known: &Known, unit: &str) -> bool {
    known.entries.iter().any(|k| k.unit == unit)
}

fn cell(text: &str) -> String {
    let text: String = text.chars().take(160).collect();
    format!("`{}`", text.replace('|', "\\|").replace('`', "'"))
}

/// The tree a build's units name, from `build.json`, or `fallback` for a build written before it
/// recorded one.
#[must_use]
pub fn tree(source: &Path, fallback: PathBuf) -> PathBuf {
    if source.as_os_str().is_empty() {
        fallback
    } else {
        source.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rk_shim::record::parse_log;

    const LOG: &str = r#"{"started":1,"argv":["/o/rk-bin/rk-cc","-Wp,-MMD,kernel/.fork.o.d","-nostdinc","-O2","-c","-o","kernel/fork.o","../kernel/fork.c"],"compiler":"/g","cwd":"/o","inputs":[{"path":"../kernel/fork.c","sha256":""}],"wall-seconds":0.1,"exit":0}
{"started":1,"argv":["/o/rk-bin/rk-cc","-Wp,-MMD,arch/x86/entry/.entry_64.o.d","-D__ASSEMBLY__","-c","-o","arch/x86/entry/entry_64.o","../arch/x86/entry/entry_64.S"],"compiler":"/g","cwd":"/o","inputs":[{"path":"../arch/x86/entry/entry_64.S","sha256":""}],"wall-seconds":0.1,"exit":0}
{"started":1,"argv":["/o/rk-bin/rk-cc","-Werror","-c","-x","c","/dev/null","-o",".tmp_1/tmp"],"compiler":"/g","cwd":"/o","wall-seconds":0.1,"exit":0,"probe":true}
"#;

    #[test]
    fn a_unit_is_replayed_without_its_output_and_with_the_front_end_mode() {
        let (records, _) = parse_log(LOG);
        let persona = ["-fgnuc-version=14.2.0".to_string()];
        assert_eq!(
            replay_args(&records[0], &persona).join(" "),
            "-nostdinc -O2 ../kernel/fork.c -fsyntax-only -fgnuc-version=14.2.0"
        );
        assert_eq!(
            replay_args(&records[1], &[]).join(" "),
            "-D__ASSEMBLY__ ../arch/x86/entry/entry_64.S -E"
        );
    }

    #[cfg(unix)]
    #[test]
    fn units_run_in_their_directory_and_probes_are_left_out() {
        let dir = std::env::temp_dir().join(format!("rk-syntax-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("o")).unwrap();
        std::fs::create_dir_all(dir.join("kernel")).unwrap();
        std::fs::write(dir.join("kernel/fork.c"), "int x;\n").unwrap();
        let log = LOG.replace("\"/o\"", &format!("\"{}\"", dir.join("o").display()));
        let (records, _) = parse_log(&log);
        // A stand-in compiler that fails on assembly and passes C.
        let fake = dir.join("cc.sh");
        std::fs::write(
            &fake,
            "#!/bin/sh\nfor a; do case $a in -E) echo \"x.S:1: error: no '.cfi' here\" >&2; exit 1;; esac; done\n[ -f ../kernel/fork.c ]\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let results = run(&records, &dir, &fake, &[], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].unit, "arch/x86/entry/entry_64.S");
        assert_eq!(results[0].error.as_deref(), Some("no '_' here"));
        assert_eq!(results[1].error, None);
        let known =
            Known::parse("[[known]]\nunit = \"arch/x86/entry/*\"\nissue = \"tamnd/rucc#2270\"\n")
                .unwrap();
        let summary = Summary::new(&results, &known);
        assert!(summary.passed());
        assert!(
            summary
                .report()
                .starts_with("2 units, 1 passed, 1 failed: 0 new and 1 known.")
        );
        assert!(!Summary::new(&results, &Known::default()).passed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_known_failure_needs_an_issue_and_says_when_it_is_fixed() {
        assert!(Known::parse("[[known]]\nunit = \"a.c\"\nissue = \"\"\n").is_err());
        let known = Known::parse("[[known]]\nunit = \"a.c\"\nissue = \"tamnd/rucc#1\"\n").unwrap();
        let passed = [Replayed {
            unit: "a.c".to_string(),
            error: None,
            line: String::new(),
        }];
        let summary = Summary::new(&passed, &known);
        assert_eq!(
            summary.fixed,
            [("a.c".to_string(), "tamnd/rucc#1".to_string())]
        );
        assert!(summary.report().contains("can come off the list"));
    }
}
