//! `rk test`: the graded test run, which boots the reference kernel and the rucc kernel, runs the
//! same suites on both and compares them unit by unit (11.7 and 11.9 in the plan).
//!
//! A unit is one thing that passes or fails: `boot` for the kernel reaching the end of init with
//! no panic, `smoke:<check>` for each smoke check, `kunit:<suite>.<case>` for each KUnit result,
//! and `kunit-module:<path>` for each test module loading. Every suite is its own boot, with
//! `rk.suite=` on the kernel command line, and the kunit boot carries the build's modules in its
//! initramfs. The units the reference passes in every run are the graded set, and the rucc kernel
//! must pass every one of them in every run. A unit the reference does not always pass is
//! reported and not graded.
//!
//! Both kernels' consoles are their dmesg. The splats in them are normalized (see `dmesg`), and a
//! splat that only the rucc kernel prints is a failure even when every unit passed.
//!
//! Attribution needs two userlands, the GCC one the reference runs and the rucc one the rucc
//! kernel runs. When they differ, every suite with a failed unit runs twice more: the rucc kernel
//! with the GCC userland, and the reference kernel with the rucc userland. A unit that still fails
//! on the rucc kernel with the GCC userland is the kernel's, and goes to `rk mixed`. One that
//! passes there but fails on the reference kernel with the rucc userland is a user program
//! miscompilation. One that passes both ways is an interaction. Until rucc builds the userland
//! both kernels run the same busybox, so every failure is the kernel's.
//!
//! The selftests are units too, `kselftest:<collection>:<program>`, one boot per collection, with
//! the programs `rk selftests` built put under `/kselftest` in the initramfs with the libraries
//! they need. They are user programs, so a rucc-built set passed with `--rucc-selftests` is part
//! of the rucc userland for attribution, the way `--rucc-busybox` is.
//!
//! LTP is the same, one boot per runtest file pinned for the row, with what `rk ltp` installed for
//! that file put under `/ltp`. Every tag is a unit, `ltp:<runtest>:<tag>`, and a rucc-built LTP
//! passed with `--rucc-ltp` is part of the rucc userland too.
//!
//! What comes out is `test.json` and `summary.md` in the run directory, next to the console of
//! every boot.

use crate::boot;
use crate::personas::Row;
use crate::tap::{self, Status};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// A kind of test `rk test` can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// The kernel boots and init finishes.
    Boot,
    /// The smoke checks of `rk-init`.
    Smoke,
    /// KUnit.
    Kunit,
    /// The kernel's selftests, one boot per collection.
    Kselftest,
    /// LTP, one boot per runtest file.
    Ltp,
}

impl Kind {
    /// The word `--kinds` takes for it.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Smoke => "smoke",
            Self::Kunit => "kunit",
            Self::Kselftest => "kselftest",
            Self::Ltp => "ltp",
        }
    }
}

/// Read `--kinds`, a comma separated list.
pub fn parse_kinds(text: &str) -> Result<Vec<Kind>, String> {
    let mut kinds = Vec::new();
    for word in text.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        let kind = match word {
            "boot" => Kind::Boot,
            "smoke" => Kind::Smoke,
            "kunit" => Kind::Kunit,
            "kselftest" => Kind::Kselftest,
            "ltp" => Kind::Ltp,
            other => {
                return Err(format!(
                    "unknown test kind {other}; the kinds are boot, smoke, kunit, kselftest and ltp"
                ));
            }
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        return Err("--kinds names no kind".to_string());
    }
    kinds.sort();
    Ok(kinds)
}

/// The suites, each one boot, that the kinds need, with one `kselftest:<collection>` suite for
/// each selftest collection and one `ltp:<runtest>` suite for each runtest file.
#[must_use]
pub fn suites(kinds: &[Kind], collections: &[String], runtests: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if kinds.contains(&Kind::Boot) || kinds.contains(&Kind::Smoke) {
        out.push("smoke".to_string());
    }
    if kinds.contains(&Kind::Kunit) {
        out.push("kunit".to_string());
    }
    if kinds.contains(&Kind::Kselftest) {
        out.extend(collections.iter().map(|c| format!("kselftest:{c}")));
    }
    if kinds.contains(&Kind::Ltp) {
        out.extend(runtests.iter().map(|r| format!("ltp:{r}")));
    }
    out
}

/// The kind a unit belongs to.
#[must_use]
pub fn kind_of(unit: &str) -> Kind {
    if unit.starts_with("smoke:") {
        Kind::Smoke
    } else if unit.starts_with("kselftest:") {
        Kind::Kselftest
    } else if unit.starts_with("ltp:") {
        Kind::Ltp
    } else if unit.starts_with("kunit") {
        Kind::Kunit
    } else {
        Kind::Boot
    }
}

/// Units by name, from one boot or one run.
pub type Units = BTreeMap<String, Status>;

/// Keep the worse result of a unit that two boots both report, which is only ever `boot`.
fn merge(into: &mut Units, from: Units) {
    for (unit, status) in from {
        let entry = into.entry(unit).or_insert(status);
        *entry = (*entry).max(status);
    }
}

/// The units one boot showed, for the kinds asked for.
#[must_use]
pub fn units_of(suite: &str, kinds: &[Kind], outcome: &boot::Outcome, console: &str) -> Units {
    let mut units = Units::new();
    let booted =
        outcome.booted.is_some() && outcome.done && outcome.panic.is_none() && !outcome.timed_out;
    units.insert(
        "boot".to_string(),
        if booted { Status::Pass } else { Status::Fail },
    );
    if suite == "smoke" {
        for (check, ok) in &outcome.checks {
            units.insert(
                format!("smoke:{check}"),
                if *ok { Status::Pass } else { Status::Fail },
            );
        }
    }
    if suite == "kunit" {
        for (path, status) in tap::parse(console) {
            units.insert(format!("kunit:{path}"), status);
        }
        for line in console.lines() {
            let line = line.trim_end_matches('\r');
            if let Some((module, result)) = line
                .strip_prefix("RK-MODULE ")
                .and_then(|rest| rest.rsplit_once(' '))
            {
                let status = if result == "pass" {
                    Status::Pass
                } else {
                    Status::Fail
                };
                units.insert(format!("kunit-module:{module}"), status);
            }
        }
    }
    if suite.starts_with("kselftest:") {
        units.extend(crate::selftests::units(console));
    }
    if suite.starts_with("ltp:") {
        units.extend(crate::ltp::units(console));
    }
    units.retain(|unit, _| kinds.contains(&kind_of(unit)));
    units
}

/// One side's results over every run: each unit's status per run.
pub type Results = BTreeMap<String, Vec<Status>>;

/// Add one run's units, the run'th, so that a unit missing from some runs is `Missing` there.
pub fn add_run(results: &mut Results, run: usize, units: &Units) {
    for (unit, status) in units {
        let row = results.entry(unit.clone()).or_default();
        row.resize(run, Status::Missing);
        row.push(*status);
    }
    for row in results.values_mut() {
        row.resize(run + 1, Status::Missing);
    }
}

/// A unit in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Unit {
    /// Its name.
    pub name: String,
    /// The reference kernel's result in each run.
    pub reference: Vec<Status>,
    /// The rucc kernel's result in each run.
    pub other: Vec<Status>,
    /// Whether it is graded: the reference passed it every time.
    pub graded: bool,
    /// Whether the rucc kernel passed it every time.
    pub passed: bool,
    /// For a graded unit the rucc kernel failed: `kernel`, `userland` or `interaction`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
}

/// Grade the rucc kernel's results against the reference's over `runs` runs.
#[must_use]
pub fn grade(reference: &Results, other: &Results, runs: usize) -> Vec<Unit> {
    let names: BTreeSet<&String> = reference.keys().chain(other.keys()).collect();
    let missing = vec![Status::Missing; runs];
    names
        .into_iter()
        .map(|name| {
            let r = reference.get(name).unwrap_or(&missing);
            let o = other.get(name).unwrap_or(&missing);
            Unit {
                name: name.clone(),
                reference: r.clone(),
                other: o.clone(),
                graded: r.len() == runs && r.iter().all(|s| *s == Status::Pass),
                passed: o.len() == runs && o.iter().all(|s| *s == Status::Pass),
                attribution: None,
            }
        })
        .collect()
}

/// Where a failure comes from, given the two cells of the 2x2 that were run for it: the rucc
/// kernel with the GCC userland, and the reference kernel with the rucc userland.
#[must_use]
pub fn attribute(
    rucc_kernel_gcc_userland: Status,
    gcc_kernel_rucc_userland: Status,
) -> &'static str {
    if rucc_kernel_gcc_userland != Status::Pass {
        "kernel"
    } else if gcc_kernel_rucc_userland != Status::Pass {
        "userland"
    } else {
        "interaction"
    }
}

/// Everything `rk test` was asked to do.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The reference build directory.
    pub reference: PathBuf,
    /// The rucc build directory.
    pub other: PathBuf,
    /// The row both were built for.
    pub row: Row,
    /// The GCC userland: a static busybox.
    pub busybox: Vec<u8>,
    /// The rucc userland, when there is one. Without it both kernels run `busybox`.
    pub rucc_busybox: Option<Vec<u8>>,
    /// The GCC-built selftests, an `rk selftests` output directory.
    pub selftests: Option<PathBuf>,
    /// The rucc-built selftests. Without them both kernels run `selftests`.
    pub rucc_selftests: Option<PathBuf>,
    /// The GCC-built LTP, an `rk ltp` output directory.
    pub ltp: Option<PathBuf>,
    /// The rucc-built LTP. Without it both kernels run `ltp`.
    pub rucc_ltp: Option<PathBuf>,
    /// The kinds of test.
    pub kinds: Vec<Kind>,
    /// How many times each kernel runs every suite.
    pub runs: usize,
    /// Seconds each boot may take.
    pub timeout: u64,
    /// The run directory.
    pub out: PathBuf,
}

/// The modules of a build as files for the initramfs, under `lib/modules/rk`, and the `order`
/// file listing them in the order kbuild built them.
pub fn modules(build: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let Ok(order) = std::fs::read_to_string(build.join("modules.order")) else {
        return Ok(Vec::new());
    };
    let mut files = Vec::new();
    let mut names = String::new();
    for line in order.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let stem = line
            .strip_suffix(".ko")
            .or_else(|| line.strip_suffix(".o"))
            .unwrap_or(line);
        let name = format!("{stem}.ko");
        let path = build.join(&name);
        let bytes = std::fs::read(&path).map_err(|e| {
            format!(
                "reading {}: {e}; build the modules too, with --targets \"<image> modules\"",
                path.display()
            )
        })?;
        files.push((format!("lib/modules/rk/{name}"), bytes));
        let _ = writeln!(names, "{name}");
    }
    if !files.is_empty() {
        files.push(("lib/modules/rk/order".to_string(), names.into_bytes()));
    }
    Ok(files)
}

impl Plan {
    /// The suites to boot, with the selftest collections and runtest files that have something
    /// built.
    fn suites(&self) -> Result<Vec<String>, String> {
        let collections = match &self.selftests {
            Some(dir) if self.kinds.contains(&Kind::Kselftest) => {
                crate::selftests::Outcome::load(dir)?.runnable()
            }
            _ => Vec::new(),
        };
        let runtests = match &self.ltp {
            Some(dir) if self.kinds.contains(&Kind::Ltp) => {
                crate::ltp::Outcome::load(dir)?.runnable()
            }
            _ => Vec::new(),
        };
        Ok(suites(&self.kinds, &collections, &runtests))
    }

    /// The GCC userland and the rucc one, which is the GCC one where nothing else was given.
    fn userlands(&self) -> (Userland<'_>, Userland<'_>) {
        let gcc = Userland {
            busybox: &self.busybox,
            selftests: self.selftests.as_deref(),
            ltp: self.ltp.as_deref(),
        };
        let rucc = Userland {
            busybox: self.rucc_busybox.as_deref().unwrap_or(&self.busybox),
            selftests: self.rucc_selftests.as_deref().or(gcc.selftests),
            ltp: self.rucc_ltp.as_deref().or(gcc.ltp),
        };
        (gcc, rucc)
    }
}

/// The kind of the units a suite reports, apart from `boot`, which every suite reports.
#[must_use]
pub fn suite_kind(suite: &str) -> Kind {
    match suite {
        "smoke" => Kind::Smoke,
        "kunit" => Kind::Kunit,
        _ if suite.starts_with("kselftest:") => Kind::Kselftest,
        _ if suite.starts_with("ltp:") => Kind::Ltp,
        _ => Kind::Boot,
    }
}

/// A userland: the busybox and, for the selftests and LTP, the output directories of
/// `rk selftests` and `rk ltp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Userland<'a> {
    busybox: &'a [u8],
    selftests: Option<&'a Path>,
    ltp: Option<&'a Path>,
}

/// One boot of one kernel with one userland running one suite, named by `stem` in the run
/// directory. The initramfs is removed after the boot, and the console is kept.
fn boot_suite(
    plan: &Plan,
    build: &Path,
    userland: Userland,
    suite: &str,
    stem: &str,
) -> Result<(Units, BTreeSet<String>), String> {
    let files = if suite == "kunit" {
        modules(build)?
    } else if let Some(collection) = suite.strip_prefix("kselftest:") {
        let dir = userland
            .selftests
            .ok_or("the kselftest kind needs --selftests, an rk selftests directory")?;
        crate::selftests::files(dir, collection)?
    } else if let Some(runtest) = suite.strip_prefix("ltp:") {
        let dir = userland
            .ltp
            .ok_or("the ltp kind needs --ltp, an rk ltp directory")?;
        crate::ltp::files(dir, runtest)?
    } else {
        Vec::new()
    };
    let timeout = if suite.starts_with("ltp:") {
        crate::ltp::timeout(plan.timeout, &files)
    } else {
        plan.timeout
    };
    let busybox = userland.busybox;
    let initramfs = plan.out.join(format!("{stem}.cpio"));
    std::fs::write(&initramfs, boot::initramfs_with(busybox, &files))
        .map_err(|e| format!("writing {}: {e}", initramfs.display()))?;
    eprintln!("rk: {stem}");
    let outcome = boot::run(&boot::Plan {
        build: build.to_path_buf(),
        row: plan.row.clone(),
        initramfs: initramfs.clone(),
        timeout,
        append: format!("rk.suite={suite}"),
        stem: Some(plan.out.join(stem)),
    });
    let _ = std::fs::remove_file(&initramfs);
    let outcome = outcome?;
    let log = plan.out.join(format!("{stem}.log"));
    let console =
        std::fs::read_to_string(&log).map_err(|e| format!("reading {}: {e}", log.display()))?;
    Ok((
        units_of(suite, &plan.kinds, &outcome, &console),
        crate::dmesg::splats(&console),
    ))
}

/// The splats of both kernels over every run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Splats {
    /// The reference kernel's.
    pub reference: BTreeSet<String>,
    /// The rucc kernel's.
    pub other: BTreeSet<String>,
    /// The rucc kernel's that the reference never printed.
    pub other_only: BTreeSet<String>,
}

/// What `rk test` found, written to `test.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// The reference build directory.
    pub reference: PathBuf,
    /// The rucc build directory.
    pub other: PathBuf,
    /// The kinds of test.
    pub kinds: Vec<Kind>,
    /// Runs of each suite on each kernel.
    pub runs: usize,
    /// Whether the rucc kernel ran its own userland, so that failures could be attributed.
    pub two_userlands: bool,
    /// Every unit either kernel reported.
    pub units: Vec<Unit>,
    /// The splats.
    pub splats: Splats,
}

impl Outcome {
    /// The graded units the rucc kernel failed.
    #[must_use]
    pub fn failed(&self) -> Vec<&Unit> {
        self.units
            .iter()
            .filter(|u| u.graded && !u.passed)
            .collect()
    }

    /// Whether the rucc kernel passed every graded unit and printed no splat of its own.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failed().is_empty() && self.splats.other_only.is_empty()
    }
}

/// Run every suite on both kernels, grade, attribute, and write `test.json` and `summary.md`.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    std::fs::create_dir_all(&plan.out)
        .map_err(|e| format!("creating {}: {e}", plan.out.display()))?;
    let (gcc, rucc) = plan.userlands();
    let two_userlands = gcc != rucc;
    let suites = plan.suites()?;
    let mut reference = Results::new();
    let mut other = Results::new();
    let mut splats = Splats::default();
    for run in 0..plan.runs {
        let (mut r_units, mut o_units) = (Units::new(), Units::new());
        for suite in &suites {
            let n = run + 1;
            let (units, found) = boot_suite(
                plan,
                &plan.reference,
                gcc,
                suite,
                &format!("reference-{}-{n}", stem(suite)),
            )?;
            merge(&mut r_units, units);
            splats.reference.extend(found);
            let (units, found) = boot_suite(
                plan,
                &plan.other,
                rucc,
                suite,
                &format!("other-{}-{n}", stem(suite)),
            )?;
            merge(&mut o_units, units);
            splats.other.extend(found);
        }
        add_run(&mut reference, run, &r_units);
        add_run(&mut other, run, &o_units);
    }
    splats.other_only = splats
        .other
        .difference(&splats.reference)
        .cloned()
        .collect();
    let mut units = grade(&reference, &other, plan.runs);

    let failed_kinds: BTreeSet<Kind> = units
        .iter()
        .filter(|u| u.graded && !u.passed)
        .map(|u| kind_of(&u.name))
        .collect();
    let (mut swapped_kernel, mut swapped_userland) = (Units::new(), Units::new());
    if two_userlands && !failed_kinds.is_empty() {
        for suite in &suites {
            if !failed_kinds.contains(&Kind::Boot) && !failed_kinds.contains(&suite_kind(suite)) {
                continue;
            }
            let (found, _) = boot_suite(
                plan,
                &plan.other,
                gcc,
                suite,
                &format!("other-gcc-userland-{}", stem(suite)),
            )?;
            merge(&mut swapped_kernel, found);
            let (found, _) = boot_suite(
                plan,
                &plan.reference,
                rucc,
                suite,
                &format!("reference-rucc-userland-{}", stem(suite)),
            )?;
            merge(&mut swapped_userland, found);
        }
    }
    for unit in units.iter_mut().filter(|u| u.graded && !u.passed) {
        unit.attribution = Some(if two_userlands {
            let cell = |units: &Units| units.get(&unit.name).copied().unwrap_or(Status::Missing);
            attribute(cell(&swapped_kernel), cell(&swapped_userland)).to_string()
        } else {
            "kernel".to_string()
        });
    }

    let outcome = Outcome {
        reference: plan.reference.clone(),
        other: plan.other.clone(),
        kinds: plan.kinds.clone(),
        runs: plan.runs,
        two_userlands,
        units,
        splats,
    };
    let json = serde_json::to_string_pretty(&outcome).unwrap_or_default();
    std::fs::write(plan.out.join("test.json"), json + "\n")
        .map_err(|e| format!("writing test.json: {e}"))?;
    std::fs::write(plan.out.join("summary.md"), summary(&outcome))
        .map_err(|e| format!("writing summary.md: {e}"))?;
    Ok(outcome)
}

/// A suite as part of a file name, without the colon of `kselftest:<collection>`.
fn stem(suite: &str) -> String {
    suite.replace(':', "-")
}

/// A unit's results over the runs, as words.
fn statuses(list: &[Status]) -> String {
    list.iter().map(|s| s.word()).collect::<Vec<_>>().join(" ")
}

/// The outcome as markdown.
#[must_use]
pub fn summary(o: &Outcome) -> String {
    let mut s = String::from("### rk test\n\n");
    let kinds: Vec<&str> = o.kinds.iter().map(|k| k.word()).collect();
    let graded = o.units.iter().filter(|u| u.graded).count();
    let failed = o.failed();
    let _ = writeln!(
        s,
        "{} over {} runs: the reference passed {graded} units every time, and the rucc kernel passed {} of them. {} splats are only in the rucc kernel's log. {}.\n",
        kinds.join(", "),
        o.runs,
        graded - failed.len(),
        o.splats.other_only.len(),
        if o.passed() { "Passed" } else { "Failed" }
    );
    if !o.two_userlands {
        s.push_str("Both kernels ran the same userland, so every failure is the kernel's.\n\n");
    }
    if !failed.is_empty() {
        s.push_str("| unit | reference | rucc | attribution |\n|---|---|---|---|\n");
        for u in failed.iter().take(200) {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                u.name.replace('|', "\\|"),
                statuses(&u.reference),
                statuses(&u.other),
                u.attribution.as_deref().unwrap_or("")
            );
        }
        if failed.len() > 200 {
            let _ = writeln!(s, "\nand {} more.", failed.len() - 200);
        }
        s.push('\n');
    }
    if !o.splats.other_only.is_empty() {
        s.push_str("Splats only in the rucc kernel's log:\n\n");
        for splat in &o.splats.other_only {
            let _ = writeln!(s, "- `{}`", splat.replace('`', "'"));
        }
        s.push('\n');
    }
    let ungraded: Vec<&Unit> = o.units.iter().filter(|u| !u.graded).collect();
    if !ungraded.is_empty() {
        let _ = writeln!(
            s,
            "{} units are not graded because the reference did not pass them every time, and the rucc kernel passed {} of those.",
            ungraded.len(),
            ungraded.iter().filter(|u| u.passed).count()
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_read_and_the_later_ones_refused() {
        assert_eq!(
            parse_kinds("kunit,boot,smoke,boot").unwrap(),
            [Kind::Boot, Kind::Smoke, Kind::Kunit]
        );
        assert_eq!(
            parse_kinds("boot,kselftest").unwrap(),
            [Kind::Boot, Kind::Kselftest]
        );
        assert_eq!(parse_kinds("ltp,boot").unwrap(), [Kind::Boot, Kind::Ltp]);
        assert!(parse_kinds("fuzz").unwrap_err().contains("unknown"));
        let timers = ["timers".to_string(), "size".to_string()];
        let syscalls = ["syscalls".to_string()];
        assert_eq!(suites(&[Kind::Boot], &timers, &syscalls), ["smoke"]);
        assert_eq!(suites(&[Kind::Kunit], &timers, &syscalls), ["kunit"]);
        assert_eq!(
            suites(&[Kind::Kselftest], &timers, &syscalls),
            ["kselftest:timers", "kselftest:size"]
        );
        assert_eq!(suites(&[Kind::Ltp], &timers, &syscalls), ["ltp:syscalls"]);
        assert_eq!(kind_of("ltp:syscalls:abort01"), Kind::Ltp);
        assert_eq!(suite_kind("ltp:syscalls"), Kind::Ltp);
        assert_eq!(kind_of("kselftest:timers:posix_timers"), Kind::Kselftest);
        assert_eq!(suite_kind("kselftest:timers"), Kind::Kselftest);
        assert_eq!(suite_kind("kunit"), Kind::Kunit);
    }

    fn booted() -> boot::Outcome {
        boot::Outcome {
            booted: Some("7.2.8".to_string()),
            done: true,
            checks: vec![("fork".to_string(), true), ("pipe".to_string(), false)],
            ..boot::Outcome::default()
        }
    }

    #[test]
    fn a_boot_gives_units_for_the_kinds_asked_for() {
        let units = units_of("smoke", &[Kind::Boot, Kind::Smoke], &booted(), "");
        assert_eq!(units["boot"], Status::Pass);
        assert_eq!(units["smoke:pipe"], Status::Fail);
        let only_boot = units_of("smoke", &[Kind::Boot], &booted(), "");
        assert_eq!(only_boot.len(), 1);
        let console = "RK-BOOTED 7.2.8 1.0\n[ 1.0]     # Subtest: list\n[ 1.0]     ok 1 list_test\n[ 1.0] ok 1 list\nRK-MODULE lib/list-test.ko pass\nRK-MODULE lib/bad.ko fail\n";
        let units = units_of("kunit", &[Kind::Kunit], &booted(), console);
        assert_eq!(units["kunit:list.list_test"], Status::Pass);
        assert_eq!(units["kunit-module:lib/bad.ko"], Status::Fail);
        assert!(!units.contains_key("boot"));
    }

    #[test]
    fn a_unit_the_reference_always_passes_is_graded() {
        let mut reference = Results::new();
        let mut other = Results::new();
        let run = |pairs: &[(&str, Status)]| -> Units {
            pairs.iter().map(|(u, s)| ((*u).to_string(), *s)).collect()
        };
        add_run(
            &mut reference,
            0,
            &run(&[
                ("boot", Status::Pass),
                ("kunit:a", Status::Pass),
                ("kunit:flaky", Status::Fail),
            ]),
        );
        add_run(
            &mut reference,
            1,
            &run(&[
                ("boot", Status::Pass),
                ("kunit:a", Status::Pass),
                ("kunit:flaky", Status::Pass),
            ]),
        );
        add_run(
            &mut other,
            0,
            &run(&[("boot", Status::Pass), ("kunit:a", Status::Pass)]),
        );
        add_run(&mut other, 1, &run(&[("boot", Status::Pass)]));
        assert_eq!(other["kunit:a"], [Status::Pass, Status::Missing]);
        let units = grade(&reference, &other, 2);
        let by_name: BTreeMap<&str, &Unit> = units.iter().map(|u| (u.name.as_str(), u)).collect();
        assert!(by_name["boot"].graded && by_name["boot"].passed);
        assert!(by_name["kunit:a"].graded && !by_name["kunit:a"].passed);
        assert!(!by_name["kunit:flaky"].graded);
    }

    #[test]
    fn failures_are_attributed_by_the_two_other_cells() {
        assert_eq!(attribute(Status::Fail, Status::Pass), "kernel");
        assert_eq!(attribute(Status::Missing, Status::Fail), "kernel");
        assert_eq!(attribute(Status::Pass, Status::Fail), "userland");
        assert_eq!(attribute(Status::Pass, Status::Pass), "interaction");
    }

    #[test]
    fn a_splat_of_its_own_fails_the_rucc_kernel() {
        let mut o = Outcome {
            reference: PathBuf::new(),
            other: PathBuf::new(),
            kinds: vec![Kind::Boot],
            runs: 1,
            two_userlands: false,
            units: Vec::new(),
            splats: Splats::default(),
        };
        assert!(o.passed());
        o.splats
            .other_only
            .insert("WARNING: CPU: N PID: N at mm/slub.c:1 f+N".to_string());
        assert!(!o.passed());
        assert!(summary(&o).contains("- `WARNING: CPU: N PID: N at mm/slub.c:1 f+N`"));
    }
}
