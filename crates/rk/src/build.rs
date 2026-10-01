//! `rk build`: one kernel, configured and built through the shim, with a record of what happened.
//!
//! The tree is never written to. kbuild's output goes under `O=`, and the compiler is the shim,
//! copied into the build directory with an `rk-cc.toml` naming the real compiler, so that every
//! sub make, including the ones that clean their environment, still goes through it. When the
//! compiler is rucc, the era's persona is part of `CC`, as `rk-cc -fgnuc-version=14.2.0`, which
//! is what a user building by hand would write too.
//!
//! Nothing else is added to the command line, with one exception that changes no code:
//! `--stack-usage` passes `KCFLAGS=-fstack-usage`, as the kernel's own `scripts/stackusage` does,
//! so that both compilers write the `.su` files `rk frames` reads.
//!
//! What comes out is `build.json`, a summary of the run that a report or a later command reads,
//! and `summary.md`, the same for a person. `compile.jsonl` holds every compiler call.

use crate::kconfig;
use crate::personas::{Era, Row};
use crate::pins::Pin;
use rk_shim::config::ShimConfig;
use rk_shim::digest::sha256_file;
use rk_shim::record::{CompileRecord, read_log};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// Everything `rk build` was asked to do.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The pinned tree.
    pub pin: Pin,
    /// Where the tree is unpacked.
    pub source: PathBuf,
    /// The row.
    pub row: Row,
    /// The era of the version.
    pub era: Era,
    /// The kbuild configuration target, as in `defconfig` or `tinyconfig`, or the name of a
    /// pinned distribution config.
    pub config: String,
    /// The pinned distribution config `config` names, and the directory it lives in.
    pub distro: Option<(crate::distro::Distro, PathBuf)>,
    /// The compiler.
    pub compiler: Compiler,
    /// kbuild's output directory.
    pub out: PathBuf,
    /// Parallel jobs.
    pub jobs: usize,
    /// Whether to carry on past failed units with `make -k`.
    pub keep_going: bool,
    /// Whether to stop after the configuration.
    pub config_only: bool,
    /// Whether the shim compiles every unit twice.
    pub twice: bool,
    /// Words for kbuild's `KCFLAGS`, which are only ever flags that change no code, such as
    /// `-fstack-usage` for `rk frames`.
    pub kcflags: Vec<String>,
    /// The bring-up classes to delegate, and to which compiler.
    pub bringup: Vec<String>,
    /// The compiler delegated calls go to.
    pub bringup_cc: Option<PathBuf>,
    /// The make targets, by default the row's image.
    pub targets: Vec<String>,
    /// A fragment to merge after the configuration target, by name and path.
    pub fragment: Option<(String, PathBuf)>,
}

/// The compiler under test or the reference, identified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Compiler {
    /// The absolute path.
    pub path: PathBuf,
    /// The first line of `--version`.
    pub version: String,
    /// The SHA-256 of the binary.
    pub sha256: String,
    /// Whether it is rucc.
    pub rucc: bool,
}

impl Compiler {
    /// Find a compiler on `PATH` or by path, and ask it who it is.
    pub fn identify(name: &str) -> Result<Self, String> {
        let path = resolve(name).ok_or_else(|| format!("no compiler {name} on PATH"))?;
        let out = Command::new(&path)
            .arg("--version")
            .output()
            .map_err(|e| format!("running {} --version: {e}", path.display()))?;
        let version = String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let sha256 = sha256_file(&path).unwrap_or_default();
        let rucc = version.contains("rucc")
            || path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("rucc"));
        Ok(Self {
            path,
            version,
            sha256,
            rucc,
        })
    }

    /// The short name used in build directory names: `rucc`, or the file name.
    #[must_use]
    pub fn label(&self) -> String {
        if self.rucc {
            return "rucc".to_string();
        }
        self.path
            .file_name()
            .map_or_else(|| "cc".to_string(), |n| n.to_string_lossy().into_owned())
    }
}

/// A name on `PATH`, or a path, made absolute.
fn resolve(name: &str) -> Option<PathBuf> {
    let direct = Path::new(name);
    if name.contains('/') {
        return std::fs::canonicalize(direct).ok();
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    })
}

/// What happened, written to `build.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// The kernel version.
    pub version: String,
    /// The row.
    pub row: String,
    /// The configuration target.
    pub config: String,
    /// The era.
    pub era: String,
    /// The era's `__GNUC__` version and default `-std=`, whatever the compiler, so that a tool
    /// reading the build can give rucc the same persona.
    #[serde(default)]
    pub gnuc: String,
    /// See `gnuc`.
    #[serde(default)]
    pub std: String,
    /// Where the kernel tree was unpacked.
    #[serde(default)]
    pub source: PathBuf,
    /// The compiler.
    pub compiler: Compiler,
    /// The arguments that were part of `CC` after the shim, which is the persona.
    pub persona: Vec<String>,
    /// The make targets.
    pub targets: Vec<String>,
    /// What the build passed in `KCFLAGS`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kcflags: Vec<String>,
    /// The fragment merged after the configuration target, if any.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fragment: String,
    /// The fragment's requests that did not take effect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fragment_missed: Vec<String>,
    /// Whether the configuration step succeeded.
    pub configured: bool,
    /// Whether the build step succeeded. False when it did not run.
    pub built: bool,
    /// The SHA-256 of the `.config` that came out, if one did.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub config_sha256: String,
    /// The SHA-256 of the boot image, if one was built.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image_sha256: String,
    /// Seconds spent configuring and building.
    pub wall_seconds: f64,
    /// Counts over `compile.jsonl`.
    pub calls: Calls,
    /// The most common error messages from failed units, normalized, with their counts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<(String, usize)>,
    /// Failed units, as the source file relative to the tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_units: Vec<String>,
    /// Whether the run can be graded: no call was delegated.
    pub graded: bool,
    /// When a step failed for a reason that is not a failed unit, the log and its last lines
    /// that are not make's own, which is where kbuild says why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<(String, Vec<String>)>,
}

/// Counts over the compile log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Calls {
    /// Every call.
    pub total: usize,
    /// Probes.
    pub probes: usize,
    /// Probes the compiler said no to.
    pub probes_failed: usize,
    /// Units: C or `.S` files compiled to objects.
    pub units: usize,
    /// Units that failed.
    pub units_failed: usize,
    /// Calls handed to the bring-up compiler.
    pub delegated: usize,
    /// Units whose second compile under `RK_TWICE` wrote different bytes.
    pub nondeterministic: usize,
    /// Lines of the log that did not parse.
    pub unreadable: usize,
}

/// Whether a record is a unit: a C or assembly source compiled to an object.
#[must_use]
pub fn is_unit(record: &CompileRecord) -> bool {
    !record.probe
        && record.argv.iter().any(|a| a == "-c")
        && record.inputs.iter().any(|i| {
            Path::new(&i.path)
                .extension()
                .is_some_and(|e| e == "c" || e == "S")
        })
}

/// Count the log.
#[must_use]
pub fn count(records: &[CompileRecord], unreadable: usize) -> Calls {
    let mut calls = Calls {
        total: records.len(),
        unreadable,
        ..Calls::default()
    };
    for record in records {
        if record.delegated.is_some() {
            calls.delegated += 1;
        }
        if record.probe {
            calls.probes += 1;
            if !record.succeeded() {
                calls.probes_failed += 1;
            }
        } else if is_unit(record) {
            calls.units += 1;
            if !record.succeeded() {
                calls.units_failed += 1;
            }
            if record.twice.as_ref().is_some_and(|t| !t.identical) {
                calls.nondeterministic += 1;
            }
        }
    }
    calls
}

/// The message of the first `error:` line in a compiler's standard error, with the things that
/// change from unit to unit taken out: quoted names and numbers. What is left groups failures by
/// cause, which is the start of the demand census.
#[must_use]
pub fn error_key(stderr: &str) -> Option<String> {
    let line = stderr.lines().find(|l| l.contains("error"))?;
    let message = line
        .split_once("error: ")
        .map_or(line, |(_, m)| m)
        .trim()
        .to_string();
    let mut out = String::new();
    let mut quote: Option<char> = None;
    for c in message.chars() {
        match (quote, c) {
            (None, '\'' | '`' | '"') => {
                quote = Some(if c == '`' { '\'' } else { c });
                out.push_str("'_'");
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, c) if c.is_ascii_digit() => {
                if !out.ends_with('N') {
                    out.push('N');
                }
            }
            (None, c) => out.push(c),
        }
    }
    Some(out.chars().take(160).collect())
}

/// The error keys of failed units, most common first.
#[must_use]
pub fn error_census(records: &[CompileRecord]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for record in records.iter().filter(|r| is_unit(r) && !r.succeeded()) {
        let key = error_key(&record.stderr).unwrap_or_else(|| {
            record.signal.map_or_else(
                || "(no error line)".to_string(),
                |s| format!("(killed by signal {s})"),
            )
        });
        *counts.entry(key).or_default() += 1;
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// The source file of a unit, relative to the tree when it is inside it.
pub fn unit_source(record: &CompileRecord, tree: &Path) -> String {
    let input = record
        .inputs
        .iter()
        .find(|i| {
            Path::new(&i.path)
                .extension()
                .is_some_and(|e| e == "c" || e == "S")
        })
        .map_or("", |i| i.path.as_str());
    // Resolve `..` by the words alone, since an out-of-tree build names its sources through it.
    let mut full = PathBuf::new();
    for part in Path::new(&record.cwd).join(input).components() {
        match part {
            std::path::Component::ParentDir => {
                full.pop();
            }
            std::path::Component::CurDir => {}
            other => full.push(other),
        }
    }
    full.strip_prefix(tree)
        .map_or_else(|_| input.to_string(), |p| p.display().to_string())
}

/// Where `rk-cc` is: next to the running `rk`.
fn shim_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("finding rk: {e}"))?;
    let shim = exe.with_file_name("rk-cc");
    if shim.is_file() {
        Ok(shim)
    } else {
        Err(format!(
            "rk-cc is not next to rk at {}; build the workspace",
            shim.display()
        ))
    }
}

/// What kbuild would otherwise take from the clock and the machine, fixed so that two builds of
/// the same inputs give the same image.
const REPRODUCIBLE: [(&str, &str); 4] = [
    ("KBUILD_BUILD_TIMESTAMP", "Thu Jan  1 00:00:00 UTC 1970"),
    ("KBUILD_BUILD_USER", "rk"),
    ("KBUILD_BUILD_HOST", "rk"),
    ("KBUILD_BUILD_VERSION", "1"),
];

/// The make command for a target, with the shim as `CC`.
fn make(plan: &Plan, cc: &str, targets: &[String], jobs: usize, keep_going: bool) -> Command {
    let mut command = make_in(
        &plan.source,
        &plan.out,
        &plan.row,
        &plan.kcflags,
        cc,
        targets,
        jobs,
    );
    if keep_going {
        command.arg("-k");
    }
    command
}

/// The make command a build ran, for a tree and an output directory, so that `rk mixed` can run
/// it again in a copy of the output directory.
pub fn make_in(
    source: &Path,
    out: &Path,
    row: &Row,
    kcflags: &[String],
    cc: &str,
    targets: &[String],
    jobs: usize,
) -> Command {
    let mut command = Command::new("make");
    command
        .arg("-C")
        .arg(source)
        .arg(format!("O={}", out.display()))
        .arg(format!("ARCH={}", row.arch))
        .arg(format!("CC={cc}"))
        .arg(format!("-j{jobs}"))
        .envs(REPRODUCIBLE);
    if !row.cross.is_empty() && !host_is(&row.arch) {
        command.arg(format!("CROSS_COMPILE={}", row.cross));
    }
    if !kcflags.is_empty() {
        command.arg(format!("KCFLAGS={}", kcflags.join(" ")));
    }
    command.args(targets);
    command
}

/// Whether the machine we run on is the row's architecture.
pub fn host_is(arch: &str) -> bool {
    matches!(
        (std::env::consts::ARCH, arch),
        ("x86_64", "x86_64" | "i386" | "x86") | ("aarch64", "arm64") | ("riscv64", "riscv")
    )
}

/// Run a command with its output going to a log file, and say whether it succeeded.
fn run_logged(mut command: Command, log: &Path) -> Result<bool, String> {
    let file =
        std::fs::File::create(log).map_err(|e| format!("creating {}: {e}", log.display()))?;
    let err = file
        .try_clone()
        .map_err(|e| format!("opening {}: {e}", log.display()))?;
    let status = command
        .stdout(file)
        .stderr(err)
        .status()
        .map_err(|e| format!("running make: {e}"))?;
    Ok(status.success())
}

/// Lay a fragment over the `.config` in the output directory and settle it with `olddefconfig`.
/// Says whether that worked, and which requests did not take.
fn merge_fragment(plan: &Plan, cc: &str, path: &Path) -> Result<(bool, Vec<String>), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let fragment = kconfig::parse_fragment(&text);
    let dot_config = plan.out.join(".config");
    let before = std::fs::read_to_string(&dot_config)
        .map_err(|e| format!("reading {}: {e}", dot_config.display()))?;
    std::fs::write(&dot_config, kconfig::merge(&before, &fragment))
        .map_err(|e| format!("writing {}: {e}", dot_config.display()))?;
    let settled = run_logged(
        make(plan, cc, &["olddefconfig".to_string()], 1, false),
        &plan.out.join("fragment.log"),
    )?;
    Ok((
        settled,
        kconfig::missed(&kconfig::load(&dot_config)?, &fragment),
    ))
}

/// The failed units, as sources relative to the tree, sorted and without repeats.
fn failed_units(records: &[CompileRecord], source: &Path) -> Vec<String> {
    let mut units: Vec<String> = records
        .iter()
        .filter(|r| is_unit(r) && !r.succeeded())
        .map(|r| unit_source(r, source))
        .collect();
    units.sort();
    units.dedup();
    units
}

/// Write the `.config` in the output directory: the configuration target, or a pinned
/// distribution config settled with `olddefconfig`, then the fragment if there is one. Says
/// whether that worked, and which fragment requests did not take.
fn configure(plan: &Plan, cc: &str) -> Result<(bool, Vec<String>), String> {
    let out = &plan.out;
    let target = match &plan.distro {
        Some((distro, dir)) => {
            let seeded = distro.seed(dir)?;
            std::fs::write(out.join(".config"), seeded)
                .map_err(|e| format!("writing {}: {e}", out.join(".config").display()))?;
            "olddefconfig".to_string()
        }
        None => plan.config.clone(),
    };
    let mut configured = run_logged(
        make(plan, cc, std::slice::from_ref(&target), 1, false),
        &out.join("config.log"),
    )? && out.join(".config").is_file();
    let mut fragment_missed = Vec::new();
    if let (true, Some((_, path))) = (configured, &plan.fragment) {
        (configured, fragment_missed) = merge_fragment(plan, cc, path)?;
    }
    Ok((configured, fragment_missed))
}

/// Configure and build, and write `build.json` and `summary.md` in the output directory.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    std::fs::create_dir_all(&plan.out)
        .map_err(|e| format!("creating {}: {e}", plan.out.display()))?;
    let out = std::fs::canonicalize(&plan.out)
        .map_err(|e| format!("resolving {}: {e}", plan.out.display()))?;
    let bin = out.join("rk-bin");
    std::fs::create_dir_all(&bin).map_err(|e| format!("creating {}: {e}", bin.display()))?;
    let shim = bin.join("rk-cc");
    std::fs::copy(shim_binary()?, &shim).map_err(|e| format!("copying rk-cc: {e}"))?;
    let log = out.join("compile.jsonl");
    let _ = std::fs::remove_file(&log);
    let config = ShimConfig {
        real: plan.compiler.path.clone(),
        log: log.clone(),
        rucc_trace: false,
        twice: plan.twice,
        bringup: plan.bringup.clone(),
        bringup_cc: plan.bringup_cc.clone().unwrap_or_default(),
    };
    std::fs::write(bin.join(rk_shim::config::FILE_NAME), config.to_toml())
        .map_err(|e| format!("writing the shim's settings: {e}"))?;

    let persona = if plan.compiler.rucc {
        vec![format!("-fgnuc-version={}", plan.era.gnuc)]
    } else {
        Vec::new()
    };
    let cc = std::iter::once(shim.display().to_string())
        .chain(persona.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");

    let plan = Plan {
        out: out.clone(),
        ..plan.clone()
    };
    let clock = Instant::now();
    let (configured, fragment_missed) = configure(&plan, &cc)?;
    let built = if configured && !plan.config_only {
        run_logged(
            make(&plan, &cc, &plan.targets, plan.jobs, plan.keep_going),
            &out.join("build.log"),
        )?
    } else {
        false
    };
    let wall_seconds = clock.elapsed().as_secs_f64();

    let (records, unreadable) = if log.is_file() {
        read_log(&log).map_err(|e| format!("reading {}: {e}", log.display()))?
    } else {
        (Vec::new(), 0)
    };
    let calls = count(&records, unreadable);
    let outcome = Outcome {
        version: plan.pin.version.clone(),
        row: plan.row.name.clone(),
        config: plan.config.clone(),
        era: plan.era.id.clone(),
        gnuc: plan.era.gnuc.clone(),
        std: plan.era.std.clone(),
        source: plan.source.clone(),
        compiler: plan.compiler.clone(),
        persona,
        targets: plan.targets.clone(),
        kcflags: plan.kcflags.clone(),
        fragment: plan
            .fragment
            .as_ref()
            .map_or_else(String::new, |(name, _)| name.clone()),
        fragment_missed,
        configured,
        built,
        config_sha256: sha256_file(&out.join(".config")).unwrap_or_default(),
        image_sha256: sha256_file(&image_path(&out, &plan.row)).unwrap_or_default(),
        wall_seconds,
        graded: calls.delegated == 0,
        errors: error_census(&records),
        stopped: stopped(
            &out,
            configured,
            built,
            plan.config_only,
            calls.units_failed,
        ),
        failed_units: failed_units(&records, &plan.source),
        calls,
    };
    let json = serde_json::to_string_pretty(&outcome).unwrap_or_default();
    std::fs::write(out.join("build.json"), json + "\n")
        .map_err(|e| format!("writing build.json: {e}"))?;
    std::fs::write(out.join("summary.md"), summary(&outcome))
        .map_err(|e| format!("writing summary.md: {e}"))?;
    Ok(outcome)
}

/// The last lines of a log that say why kbuild stopped: not make's own lines, and not kbuild's
/// quiet progress lines, which are indented.
#[must_use]
pub fn failure_lines(log: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| {
            !l.trim().is_empty()
                && !l.starts_with("make:")
                && !l.starts_with("make[")
                && !l.starts_with("  ")
        })
        .collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| (*l).to_string())
        .collect()
}

/// Why a build stopped, when no failed unit explains it.
fn stopped(
    out: &Path,
    configured: bool,
    built: bool,
    config_only: bool,
    units_failed: usize,
) -> Option<(String, Vec<String>)> {
    let log = if !configured {
        "config.log"
    } else if !built && !config_only && units_failed == 0 {
        "build.log"
    } else {
        return None;
    };
    let text = std::fs::read_to_string(out.join(log)).ok()?;
    Some((log.to_string(), failure_lines(&text, 6)))
}

/// The summary for a person, in markdown.
#[must_use]
pub fn summary(o: &Outcome) -> String {
    let mut s = String::new();
    let state = |ok: bool| if ok { "yes" } else { "no" };
    let _ = writeln!(
        s,
        "### {} {} {} with {}\n",
        o.version,
        o.row,
        o.config,
        o.compiler.label()
    );
    let _ = writeln!(s, "| | |\n|---|---|");
    let _ = writeln!(s, "| compiler | `{}` |", o.compiler.version);
    if !o.persona.is_empty() {
        let _ = writeln!(s, "| persona | `{}` ({}) |", o.persona.join(" "), o.era);
    }
    let _ = writeln!(s, "| configured | {} |", state(o.configured));
    if !o.fragment.is_empty() {
        let missed = if o.fragment_missed.is_empty() {
            "every request took".to_string()
        } else {
            format!("did not take: {}", o.fragment_missed.join(", "))
        };
        let _ = writeln!(s, "| fragment {} | {missed} |", o.fragment);
    }
    let _ = writeln!(
        s,
        "| built `{}` | {} |",
        o.targets.join(" "),
        state(o.built)
    );
    let _ = writeln!(
        s,
        "| probes | {} ({} said no) |",
        o.calls.probes, o.calls.probes_failed
    );
    let _ = writeln!(
        s,
        "| units | {} compiled, {} failed |",
        o.calls.units - o.calls.units_failed,
        o.calls.units_failed
    );
    if o.calls.nondeterministic > 0 {
        let _ = writeln!(
            s,
            "| nondeterministic units | {} |",
            o.calls.nondeterministic
        );
    }
    let _ = writeln!(s, "| graded | {} |", state(o.graded));
    let _ = writeln!(s, "| minutes | {:.1} |", o.wall_seconds / 60.0);
    if !o.errors.is_empty() {
        let _ = writeln!(s, "\nThe most common errors in failed units:\n");
        let _ = writeln!(s, "| units | error |\n|---|---|");
        for (message, n) in o.errors.iter().take(25) {
            let _ = writeln!(s, "| {n} | `{}` |", message.replace('|', "\\|"));
        }
    }
    if let Some((log, lines)) = &o.stopped {
        let _ = writeln!(s, "\nThe end of {log}:\n\n```\n{}\n```", lines.join("\n"));
    }
    s
}

/// Where kbuild leaves a row's boot image in an output directory.
#[must_use]
pub fn image_path(out: &Path, row: &Row) -> PathBuf {
    let srcarch = match row.arch.as_str() {
        "x86_64" | "i386" => "x86",
        other => other,
    };
    out.join("arch").join(srcarch).join("boot").join(&row.image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rk_shim::record::parse_log;

    const LOG: &str = r#"{"started":1,"argv":["rk-cc","-Werror","-c","-x","c","/dev/null","-o",".tmp_1/tmp"],"compiler":"/r","cwd":"/o","wall-seconds":0.1,"exit":1,"probe":true}
{"started":1,"argv":["rk-cc","-c","-o","kernel/fork.o","/src/kernel/fork.c"],"compiler":"/r","cwd":"/o","inputs":[{"path":"/src/kernel/fork.c","sha256":"a"}],"wall-seconds":0.1,"exit":0,"twice":{"identical":false,"differing":["kernel/fork.o"]}}
{"started":1,"argv":["rk-cc","-c","-o","kernel/exit.o","/src/kernel/exit.c"],"compiler":"/r","cwd":"/o","inputs":[{"path":"/src/kernel/exit.c","sha256":"a"}],"wall-seconds":0.1,"exit":1,"stderr":"/src/kernel/exit.c:12:3: error: unknown attribute 'section' on line 40\n"}
{"started":1,"argv":["rk-cc","-c","-o","mm/slub.o","/src/mm/slub.c"],"compiler":"/r","cwd":"/o","inputs":[{"path":"/src/mm/slub.c","sha256":"a"}],"wall-seconds":0.1,"exit":1,"stderr":"/src/mm/slub.c:99:1: error: unknown attribute 'cold' on line 7\n"}
{"started":1,"argv":["rk-cc","-m16","-c","-o","arch/x86/boot/a20.o","/src/arch/x86/boot/a20.c"],"compiler":"/g","cwd":"/o","inputs":[{"path":"/src/arch/x86/boot/a20.c","sha256":"a"}],"wall-seconds":0.1,"exit":0,"delegated":"m16"}
"#;

    #[test]
    fn the_reason_kbuild_stopped_is_not_make_noise() {
        let log = "  HOSTLD  scripts/kconfig/conf
/tmp/out/rk-bin/rk-cc -fgnuc-version=14.2.0: unknown assembler invoked
scripts/Kconfig.include:51: Sorry, this assembler is not supported.
make[5]: *** [scripts/kconfig/Makefile:85: allnoconfig] Error 1
make: *** [Makefile:248: __sub-make] Error 2
make: Leaving directory '/src/linux-7.2.8'
";
        assert_eq!(
            failure_lines(log, 6),
            vec![
                "/tmp/out/rk-bin/rk-cc -fgnuc-version=14.2.0: unknown assembler invoked",
                "scripts/Kconfig.include:51: Sorry, this assembler is not supported."
            ]
        );
        assert_eq!(failure_lines(log, 1).len(), 1);
    }

    #[test]
    fn the_log_is_counted_by_kind() {
        let (records, skipped) = parse_log(LOG);
        let calls = count(&records, skipped);
        assert_eq!(calls.total, 5);
        assert_eq!(calls.probes, 1);
        assert_eq!(calls.probes_failed, 1);
        assert_eq!(calls.units, 4);
        assert_eq!(calls.units_failed, 2);
        assert_eq!(calls.delegated, 1);
        assert_eq!(calls.nondeterministic, 1);
    }

    #[test]
    fn errors_group_by_message_without_names_or_numbers() {
        let (records, _) = parse_log(LOG);
        let census = error_census(&records);
        assert_eq!(census, [("unknown attribute '_' on line N".to_string(), 2)]);
    }

    #[test]
    fn failed_units_are_named_inside_the_tree() {
        let (records, _) = parse_log(LOG);
        assert_eq!(unit_source(&records[2], Path::new("/src")), "kernel/exit.c");
    }

    #[test]
    fn a_line_without_error_gives_no_key() {
        assert_eq!(error_key("warning: unused"), None);
        assert_eq!(
            error_key("t.c:1:1: error: expected ';'").as_deref(),
            Some("expected '_'")
        );
    }
}
