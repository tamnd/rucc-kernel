//! `rk mixed`: find the object, and then the transformation, that makes a unit fail (11.8 in the
//! plan).
//!
//! Both builds follow the same ABI, so a kernel can be linked from any mix of their objects. A
//! trial is a copy of the reference build with some objects replaced by the rucc build's, relinked
//! by the reference's own make command and booted into the suite the unit belongs to. The trial
//! fails when the unit does not pass. Delta debugging over the objects that differ finds a smallest
//! set of rucc objects that still fails, which is one object when one object is responsible.
//!
//! When it is one object and the rucc build was made by rucc, the object is compiled again with
//! `-fpass-fuel-global=N` and the trials continue on the fuel. The first N that fails names one
//! transformation. The IR dumps at N - 1 and at N are compared, and the first pass whose output
//! differs, and the functions it changed there, go into the finding.
//!
//! The relink only redoes what depends on the objects: each swapped object is newer than its
//! sources and keeps the reference's `.cmd` file, so kbuild archives and links again and compiles
//! nothing. For that the make command has to be the reference's exactly, including the path of its
//! shim, which then logs the few calls the link makes into the reference's `compile.jsonl`. Those
//! lines are moved into the trial's `relink.jsonl` after every trial, so the reference's log stays
//! what its build wrote.
//!
//! A trial whose link fails is a finding of its own, since a rucc object that cannot be linked
//! with GCC objects has an ABI or section mismatch. `rk mixed` records it and stops.
//!
//! Everything goes into `mixed.json` and `summary.md` in the run directory, with the console of
//! every boot and the dumps of the fuel search.

use crate::boot;
use crate::build;
use crate::personas::Row;
use crate::tap::Status;
use crate::testrun::{self, Kind};
use rk_shim::digest::sha256_bytes;
use rk_shim::record::CompileRecord;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// Delta debugging, `ddmin`: a smallest subset of `items` for which `fails` is still true, where
/// removing any one item makes it pass. `fails` must be true for all of `items`.
///
/// The set is split into `n` parts, starting at two. A part that fails on its own becomes the set.
/// Failing that, a complement that fails does, with one part fewer. When neither does, the parts
/// get smaller, until each is one item and no complement fails. A single responsible item takes
/// about two trials per halving.
pub fn ddmin<T: Clone, E>(
    items: &[T],
    fails: &mut dyn FnMut(&[T]) -> Result<bool, E>,
) -> Result<Vec<T>, E> {
    let mut set = items.to_vec();
    let mut n = 2;
    while set.len() >= 2 {
        let parts = split(&set, n);
        let mut reduced = false;
        for part in &parts {
            if fails(part)? {
                set.clone_from(part);
                n = 2;
                reduced = true;
                break;
            }
        }
        if !reduced && n > 2 {
            for skip in 0..parts.len() {
                let complement: Vec<T> = parts
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != skip)
                    .flat_map(|(_, p)| p.iter().cloned())
                    .collect();
                if fails(&complement)? {
                    set = complement;
                    n = (n - 1).max(2);
                    reduced = true;
                    break;
                }
            }
        }
        if !reduced {
            if n >= set.len() {
                break;
            }
            n = (n * 2).min(set.len());
        }
    }
    Ok(set)
}

/// `items` in `n` parts of nearly equal length, in order.
fn split<T: Clone>(items: &[T], n: usize) -> Vec<Vec<T>> {
    let n = n.clamp(1, items.len().max(1));
    let (size, extra) = (items.len() / n, items.len() % n);
    let mut parts = Vec::with_capacity(n);
    let mut start = 0;
    for i in 0..n {
        let len = size + usize::from(i < extra);
        parts.push(items[start..start + len].to_vec());
        start += len;
    }
    parts
}

/// The smallest fuel in `good..=bad` for which `fails` is true, given that `good` passes and `bad`
/// fails. Fuel only ever adds transformations, so there is one step where it starts failing.
pub fn bisect_fuel<E>(
    mut good: u32,
    mut bad: u32,
    fails: &mut dyn FnMut(u32) -> Result<bool, E>,
) -> Result<u32, E> {
    while bad - good > 1 {
        let mid = good + (bad - good) / 2;
        if fails(mid)? {
            bad = mid;
        } else {
            good = mid;
        }
    }
    Ok(bad)
}

/// The functions of an IR dump, by name, with their text.
#[must_use]
pub fn functions(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut current: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("func @") {
            if let Some((name, body)) = current.take() {
                out.insert(name, body);
            }
            let name: String = rest
                .chars()
                .take_while(|c| *c != '(' && !c.is_whitespace())
                .collect();
            current = Some((name, String::new()));
        }
        if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some((name, body)) = current {
        out.insert(name, body);
    }
    out
}

/// One dump file of a `-fdump-ir=all` run: its position in the pipeline and its pass, read off a
/// name like `fork.c.07-after-sroa.ir`. Only the `after` dumps are kept.
#[must_use]
pub fn dump_step(file_name: &str) -> Option<(u32, String)> {
    let stem = file_name.strip_suffix(".ir")?;
    let (_, tail) = stem.rsplit_once('.')?;
    let (number, rest) = tail.split_once('-')?;
    let pass = rest.strip_prefix("after-")?;
    Some((number.parse().ok()?, pass.to_string()))
}

/// Where two dump runs first differ: the pass, and the functions whose text it changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Divergence {
    /// The pass, as `-fdump-ir` names it.
    pub pass: String,
    /// Its position in the pipeline.
    pub step: u32,
    /// Functions whose text differs after it, or that only one side has.
    pub functions: Vec<String>,
}

/// The first `after` dump where `good` and `bad` differ. Both map a dump's file name to its text.
#[must_use]
pub fn first_divergence(
    good: &BTreeMap<String, String>,
    bad: &BTreeMap<String, String>,
) -> Option<Divergence> {
    let steps = |dumps: &BTreeMap<String, String>| -> BTreeMap<(u32, String), String> {
        dumps
            .iter()
            .filter_map(|(name, text)| dump_step(name).map(|step| (step, text.clone())))
            .collect()
    };
    let (good, bad) = (steps(good), steps(bad));
    let keys: BTreeSet<&(u32, String)> = good.keys().chain(bad.keys()).collect();
    for key in keys {
        let (left, right) = (good.get(key), bad.get(key));
        if left == right {
            continue;
        }
        let empty = BTreeMap::new();
        let left = left.map_or_else(|| empty.clone(), |t| functions(t));
        let right = right.map_or(empty, |t| functions(t));
        let names: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
        let changed = names
            .into_iter()
            .filter(|name| left.get(*name) != right.get(*name))
            .cloned()
            .collect();
        return Some(Divergence {
            pass: key.1.clone(),
            step: key.0,
            functions: changed,
        });
    }
    None
}

/// The objects a trial can swap: every object the rucc build compiled from a unit, relative to its
/// output directory, that the reference has at the same path with different bytes. Each comes
/// with the rucc build's record, which the fuel search compiles again.
pub fn candidates(
    reference: &Path,
    other: &Path,
    records: &[CompileRecord],
) -> Vec<(String, CompileRecord)> {
    let mut out = BTreeMap::new();
    for record in records
        .iter()
        .filter(|r| build::is_unit(r) && r.succeeded())
    {
        let Some(object) = output_of(&record.argv) else {
            continue;
        };
        let path = Path::new(&record.cwd).join(&object);
        let Ok(relative) = path.strip_prefix(other) else {
            continue;
        };
        let relative = relative.to_string_lossy().into_owned();
        let (Ok(ours), Ok(theirs)) = (
            std::fs::read(other.join(&relative)),
            std::fs::read(reference.join(&relative)),
        ) else {
            continue;
        };
        if ours != theirs {
            out.insert(relative, record.clone());
        }
    }
    out.into_iter().collect()
}

/// The value of `-o` on a command line.
#[must_use]
pub fn output_of(argv: &[String]) -> Option<String> {
    let mut words = argv.iter();
    while let Some(word) = words.next() {
        if word == "-o" {
            return words.next().cloned();
        }
        if let Some(rest) = word.strip_prefix("-o")
            && !rest.is_empty()
        {
            return Some(rest.to_string());
        }
    }
    None
}

/// The rucc build's compile of one object again, with rucc run directly, the object written to
/// `object`, the dependency file left out and `extra` added. The words come after the program.
#[must_use]
pub fn recompile_args(record: &CompileRecord, object: &Path, extra: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut words = record.argv.iter().skip(1);
    while let Some(word) = words.next() {
        if word == "-o" {
            words.next();
            continue;
        }
        if (word.starts_with("-o") && word.len() > 2)
            || word.starts_with("-Wp,-MMD,")
            || word.starts_with("-Wp,-MD,")
        {
            continue;
        }
        out.push(word.clone());
    }
    out.push("-o".to_string());
    out.push(object.display().to_string());
    out.extend(extra.iter().cloned());
    out
}

/// The suite a unit is reported by.
#[must_use]
pub fn suite_of(unit: &str) -> String {
    match testrun::kind_of(unit) {
        Kind::Kunit => "kunit".to_string(),
        Kind::Boot | Kind::Smoke => "smoke".to_string(),
        Kind::Kselftest => {
            let collection = unit
                .strip_prefix("kselftest:")
                .and_then(|rest| rest.split(':').next())
                .unwrap_or_default();
            format!("kselftest:{collection}")
        }
        Kind::Ltp => {
            let runtest = unit
                .strip_prefix("ltp:")
                .and_then(|rest| rest.split(':').next())
                .unwrap_or_default();
            format!("ltp:{runtest}")
        }
    }
}

/// Everything `rk mixed` was asked to do.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The reference build directory.
    pub reference: PathBuf,
    /// The rucc build directory.
    pub other: PathBuf,
    /// The row both were built for.
    pub row: Row,
    /// The unit that fails, as `rk test` names it.
    pub unit: String,
    /// The userland, a static busybox.
    pub busybox: Vec<u8>,
    /// The selftests, an `rk selftests` directory, for a `kselftest:` unit.
    pub selftests: Option<PathBuf>,
    /// LTP, an `rk ltp` directory, for an `ltp:` unit.
    pub ltp: Option<PathBuf>,
    /// Seconds each boot may take.
    pub timeout: u64,
    /// Parallel jobs for the relink.
    pub jobs: usize,
    /// Whether to search the fuel once one object is left.
    pub fuel: bool,
    /// The run directory.
    pub out: PathBuf,
}

/// One trial.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Trial {
    /// Its number, which names its console.
    pub number: usize,
    /// The objects that came from the rucc build.
    pub rucc: Vec<String>,
    /// The fuel the swapped object was compiled with, in the fuel search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel: Option<u32>,
    /// Whether the unit failed.
    pub failed: bool,
    /// Whether the answer came from an earlier trial with the same bytes.
    pub cached: bool,
    /// Seconds for the relink and the boot.
    pub seconds: f64,
}

/// What the fuel search found.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Fuel {
    /// The most fuel that passes.
    pub good: u32,
    /// The least fuel that fails, which is `good + 1`.
    pub bad: u32,
    /// Where the dumps at the two differ, when they do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub divergence: Option<Divergence>,
    /// Why the search stopped early, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

/// What `rk mixed` found, written to `mixed.json`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// The reference build directory.
    pub reference: PathBuf,
    /// The rucc build directory.
    pub other: PathBuf,
    /// The unit.
    pub unit: String,
    /// How many objects differ between the builds.
    pub candidates: usize,
    /// The smallest set of rucc objects that still fails.
    pub objects: Vec<String>,
    /// The fuel search, when it ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel: Option<Fuel>,
    /// Set when a trial did not link, with the objects it had from rucc and the last lines of
    /// the log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_failure: Option<(Vec<String>, Vec<String>)>,
    /// Why the search did not start, when it did not: the reference fails the unit, or the rucc
    /// build passes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_reproduced: Option<String>,
    /// Every trial.
    pub trials: Vec<Trial>,
}

/// A trial that could not be run to an answer, which ends the search.
enum Stop {
    /// The relink failed.
    Link(Vec<String>, Vec<String>),
    /// Anything else, as a message.
    Error(String),
}

impl From<String> for Stop {
    fn from(message: String) -> Self {
        Self::Error(message)
    }
}

/// The trial tree and what is in it.
struct Trials<'a> {
    plan: &'a Plan,
    /// The copy of the reference being relinked.
    tree: PathBuf,
    /// The reference's make command.
    make: Command,
    /// The reference's compile log, which the link appends to.
    log: PathBuf,
    /// Each swapped object's current bytes, by path, where they are not the reference's.
    current: BTreeMap<String, Vec<u8>>,
    /// Answers by the digest of every swapped object's bytes.
    seen: BTreeMap<Vec<(String, String)>, bool>,
    done: Vec<Trial>,
}

impl Trials<'_> {
    /// Relink with `swapped` in place of the reference's objects, boot, and say whether the unit
    /// failed. Every object not in `swapped` is the reference's.
    fn run(
        &mut self,
        swapped: &BTreeMap<String, Vec<u8>>,
        fuel: Option<u32>,
    ) -> Result<bool, Stop> {
        let key: Vec<(String, String)> = swapped
            .iter()
            .map(|(path, bytes)| (path.clone(), sha256_bytes(bytes)))
            .collect();
        let number = self.done.len() + 1;
        if let Some(&failed) = self.seen.get(&key) {
            self.done.push(Trial {
                number,
                rucc: swapped.keys().cloned().collect(),
                fuel,
                failed,
                cached: true,
                seconds: 0.0,
            });
            return Ok(failed);
        }
        let clock = Instant::now();
        let paths: BTreeSet<String> = self.current.keys().chain(swapped.keys()).cloned().collect();
        for path in paths {
            let want = match swapped.get(&path) {
                Some(bytes) => bytes.clone(),
                None => read(&self.plan.reference.join(&path))?,
            };
            if self.current.get(&path) == Some(&want)
                || (!swapped.contains_key(&path) && !self.current.contains_key(&path))
            {
                continue;
            }
            let target = self.tree.join(&path);
            let _ = std::fs::remove_file(&target);
            std::fs::write(&target, &want)
                .map_err(|e| format!("writing {}: {e}", target.display()))?;
            if swapped.contains_key(&path) {
                self.current.insert(path, want);
            } else {
                self.current.remove(&path);
            }
        }
        let stem = self.plan.out.join(format!("trial-{number}"));
        let linked = self.relink(&stem)?;
        if !linked {
            let log = std::fs::read_to_string(stem.with_extension("make.log")).unwrap_or_default();
            return Err(Stop::Link(
                swapped.keys().cloned().collect(),
                build::failure_lines(&log, 20),
            ));
        }
        let failed = self.boot(&stem)?;
        self.seen.insert(key, failed);
        self.done.push(Trial {
            number,
            rucc: swapped.keys().cloned().collect(),
            fuel,
            failed,
            cached: false,
            seconds: clock.elapsed().as_secs_f64(),
        });
        eprintln!(
            "rk: trial {number}: {} rucc object(s){}, {}",
            swapped.len(),
            fuel.map_or_else(String::new, |f| format!(" at fuel {f}")),
            if failed { "fails" } else { "passes" }
        );
        Ok(failed)
    }

    /// Run the reference's make command in the trial tree, and move what the shim logged back out
    /// of the reference's compile log.
    fn relink(&mut self, stem: &Path) -> Result<bool, String> {
        let before = std::fs::metadata(&self.log).map_or(0, |m| m.len());
        let log = stem.with_extension("make.log");
        let file =
            std::fs::File::create(&log).map_err(|e| format!("creating {}: {e}", log.display()))?;
        let err = file.try_clone().map_err(|e| e.to_string())?;
        let status = self
            .make
            .stdout(file)
            .stderr(err)
            .status()
            .map_err(|e| format!("running make: {e}"))?;
        if let Ok(all) = std::fs::read(&self.log) {
            let start = usize::try_from(before).unwrap_or(usize::MAX).min(all.len());
            if start < all.len() {
                use std::io::Write as _;
                let moved = self.plan.out.join("relink.jsonl");
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&moved)
                    .map_err(|e| format!("opening {}: {e}", moved.display()))?;
                file.write_all(&all[start..])
                    .map_err(|e| format!("writing {}: {e}", moved.display()))?;
                std::fs::write(&self.log, &all[..start])
                    .map_err(|e| format!("writing {}: {e}", self.log.display()))?;
            }
        }
        Ok(status.success())
    }

    /// Boot the trial tree into the unit's suite and say whether the unit failed.
    fn boot(&self, stem: &Path) -> Result<bool, String> {
        let suite = suite_of(&self.plan.unit);
        let files = if suite == "kunit" {
            testrun::modules(&self.tree)?
        } else if let Some(collection) = suite.strip_prefix("kselftest:") {
            let dir = self
                .plan
                .selftests
                .as_deref()
                .ok_or("a kselftest unit needs --selftests, an rk selftests directory")?;
            crate::selftests::files(dir, collection)?
        } else if let Some(runtest) = suite.strip_prefix("ltp:") {
            let dir = self
                .plan
                .ltp
                .as_deref()
                .ok_or("an ltp unit needs --ltp, an rk ltp directory")?;
            crate::ltp::files(dir, runtest)?
        } else {
            Vec::new()
        };
        let initramfs = stem.with_extension("cpio");
        std::fs::write(&initramfs, boot::initramfs_with(&self.plan.busybox, &files))
            .map_err(|e| format!("writing {}: {e}", initramfs.display()))?;
        let outcome = boot::run(&boot::Plan {
            build: self.tree.clone(),
            row: self.plan.row.clone(),
            initramfs: initramfs.clone(),
            timeout: if suite.starts_with("ltp:") {
                crate::ltp::timeout(self.plan.timeout, &files)
            } else {
                self.plan.timeout
            },
            append: format!("rk.suite={suite}"),
            stem: Some(stem.to_path_buf()),
        });
        let _ = std::fs::remove_file(&initramfs);
        let outcome = outcome?;
        let console = std::fs::read_to_string(stem.with_extension("log")).unwrap_or_default();
        let kind = testrun::kind_of(&self.plan.unit);
        let units = testrun::units_of(&suite, &[kind], &outcome, &console);
        Ok(units.get(&self.plan.unit) != Some(&Status::Pass))
    }
}

/// Read a file whole.
fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))
}

/// The build.json of a build directory.
fn outcome_of(dir: &Path) -> Result<build::Outcome, String> {
    let path = dir.join("build.json");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("reading {}: {e}", path.display()))
}

/// Run the search and write `mixed.json` and `summary.md`.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    std::fs::create_dir_all(&plan.out)
        .map_err(|e| format!("creating {}: {e}", plan.out.display()))?;
    let reference = outcome_of(&plan.reference)?;
    let other = outcome_of(&plan.other)?;
    let log = plan.other.join("compile.jsonl");
    let (records, _) =
        rk_shim::record::read_log(&log).map_err(|e| format!("reading {}: {e}", log.display()))?;
    let found = candidates(&plan.reference, &plan.other, &records);
    let objects: Vec<String> = found.iter().map(|(path, _)| path.clone()).collect();
    let records: BTreeMap<String, CompileRecord> = found.into_iter().collect();
    eprintln!("rk: {} objects differ between the builds", objects.len());

    let tree = plan.out.join("tree");
    if tree.exists() {
        std::fs::remove_dir_all(&tree).map_err(|e| format!("clearing {}: {e}", tree.display()))?;
    }
    copy_tree(&plan.reference, &tree)?;
    let shim = plan.reference.join("rk-bin").join("rk-cc");
    let cc = std::iter::once(shim.display().to_string())
        .chain(reference.persona.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let make = build::make_in(
        &reference.source,
        &tree,
        &plan.row,
        &reference.kcflags,
        &cc,
        &reference.targets,
        plan.jobs,
    );
    let mut trials = Trials {
        plan,
        tree,
        make,
        log: plan.reference.join("compile.jsonl"),
        current: BTreeMap::new(),
        seen: BTreeMap::new(),
        done: Vec::new(),
    };
    let mut outcome = Outcome {
        reference: plan.reference.clone(),
        other: plan.other.clone(),
        unit: plan.unit.clone(),
        candidates: objects.len(),
        objects: Vec::new(),
        fuel: None,
        link_failure: None,
        not_reproduced: None,
        trials: Vec::new(),
    };
    let result = search(&mut trials, &objects, &records, &other, &mut outcome);
    outcome.trials = std::mem::take(&mut trials.done);
    match result {
        Ok(()) => {}
        Err(Stop::Link(objects, lines)) => outcome.link_failure = Some((objects, lines)),
        Err(Stop::Error(message)) => {
            write(plan, &outcome)?;
            return Err(message);
        }
    }
    write(plan, &outcome)?;
    Ok(outcome)
}

/// The two steps: the objects, and then the fuel in the one object left.
fn search(
    trials: &mut Trials<'_>,
    objects: &[String],
    records: &BTreeMap<String, CompileRecord>,
    other: &build::Outcome,
    outcome: &mut Outcome,
) -> Result<(), Stop> {
    let bytes_of = |set: &[String]| -> Result<BTreeMap<String, Vec<u8>>, Stop> {
        set.iter()
            .map(|path| Ok((path.clone(), read(&trials.plan.other.join(path))?)))
            .collect()
    };
    if trials.run(&BTreeMap::new(), None)? {
        outcome.not_reproduced = Some("the reference fails the unit on its own".to_string());
        return Ok(());
    }
    let all = bytes_of(objects)?;
    if !trials.run(&all, None)? {
        outcome.not_reproduced =
            Some("the reference with every rucc object passes the unit".to_string());
        return Ok(());
    }
    let mut fails = |set: &[String]| -> Result<bool, Stop> {
        let swapped = bytes_of(set)?;
        trials.run(&swapped, None)
    };
    outcome.objects = ddmin(objects, &mut fails)?;
    if !trials.plan.fuel || outcome.objects.len() != 1 || !other.compiler.rucc {
        return Ok(());
    }
    let object = outcome.objects[0].clone();
    let Some(record) = records.get(&object) else {
        return Ok(());
    };
    let assembly = record
        .inputs
        .iter()
        .any(|i| Path::new(&i.path).extension().is_some_and(|e| e == "S"));
    outcome.fuel = Some(if assembly {
        Fuel {
            good: 0,
            bad: 0,
            divergence: None,
            stopped: Some(
                "the object is assembled from a .S file, so there is no pass to search".to_string(),
            ),
        }
    } else {
        fuel_search(trials, &object, record, other)?
    });
    Ok(())
}

/// Compile the one object again with less and less fuel, find the step where it starts failing,
/// and compare the dumps on each side of it.
fn fuel_search(
    trials: &mut Trials<'_>,
    object: &str,
    record: &CompileRecord,
    other: &build::Outcome,
) -> Result<Fuel, Stop> {
    let work = trials.plan.out.join("fuel");
    std::fs::create_dir_all(&work).map_err(|e| format!("creating {}: {e}", work.display()))?;
    let rucc = other.compiler.path.clone();
    let compile = |fuel: u32, dump: Option<&Path>| -> Result<Vec<u8>, String> {
        let target = work.join(format!("fuel-{fuel}.o"));
        let mut extra = vec![format!("-fpass-fuel-global={fuel}")];
        if dump.is_some() {
            extra.push("-fdump-ir=all".to_string());
        }
        let cwd = Path::new(&record.cwd);
        let before: BTreeSet<PathBuf> = listing(cwd);
        let done = Command::new(&rucc)
            .args(recompile_args(record, &target, &extra))
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("running {}: {e}", rucc.display()))?;
        if let Some(into) = dump {
            std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
            for made in listing(cwd).difference(&before) {
                if made.extension().is_some_and(|e| e == "ir")
                    && let Some(name) = made.file_name()
                {
                    let _ = std::fs::rename(made, into.join(name));
                }
            }
        }
        if !done.status.success() {
            return Err(format!(
                "compiling {object} at fuel {fuel} failed: {}",
                String::from_utf8_lossy(&done.stderr).trim()
            ));
        }
        read(&target)
    };
    let mut fuel = Fuel {
        good: 0,
        bad: u32::MAX,
        divergence: None,
        stopped: None,
    };
    let mut fails = |n: u32| -> Result<bool, Stop> {
        let bytes = compile(n, None)?;
        let swapped: BTreeMap<String, Vec<u8>> = [(object.to_string(), bytes)].into();
        trials.run(&swapped, Some(n))
    };
    if fails(0)? {
        fuel.stopped = Some(
            "the object fails with no transformation at all, so the fault is in lowering or \
             code generation rather than a pass"
                .to_string(),
        );
        return Ok(fuel);
    }
    if !fails(u32::MAX)? {
        fuel.stopped = Some(
            "the object compiled again passes, so the failure does not reproduce from its record"
                .to_string(),
        );
        return Ok(fuel);
    }
    fuel.bad = bisect_fuel(0, u32::MAX, &mut fails)?;
    fuel.good = fuel.bad - 1;
    let (good_dir, bad_dir) = (work.join("dump-good"), work.join("dump-bad"));
    compile(fuel.good, Some(&good_dir))?;
    compile(fuel.bad, Some(&bad_dir))?;
    fuel.divergence = first_divergence(&dumps(&good_dir), &dumps(&bad_dir));
    Ok(fuel)
}

/// The files directly in a directory.
fn listing(dir: &Path) -> BTreeSet<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}

/// The dumps in a directory, by file name.
fn dumps(dir: &Path) -> BTreeMap<String, String> {
    listing(dir)
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some((name, std::fs::read_to_string(&path).ok()?))
        })
        .collect()
}

/// Copy a build directory, sharing blocks where the filesystem can. A hard link is not enough,
/// because kbuild rewrites some files in place, `System.map` among them, and that would write
/// into the reference.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let status = Command::new("cp")
        .arg("-a")
        .arg("--reflink=auto")
        .arg(from)
        .arg(to)
        .status()
        .map_err(|e| format!("running cp: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "copying {} to {} failed",
            from.display(),
            to.display()
        ))
    }
}

/// Write `mixed.json` and `summary.md`.
fn write(plan: &Plan, outcome: &Outcome) -> Result<(), String> {
    let json = serde_json::to_string_pretty(outcome).unwrap_or_default();
    std::fs::write(plan.out.join("mixed.json"), json + "\n")
        .map_err(|e| format!("writing mixed.json: {e}"))?;
    std::fs::write(plan.out.join("summary.md"), summary(outcome))
        .map_err(|e| format!("writing summary.md: {e}"))
}

/// Whether the search ended with an answer.
impl Outcome {
    #[must_use]
    pub fn found(&self) -> bool {
        self.link_failure.is_some() || (!self.objects.is_empty() && self.not_reproduced.is_none())
    }
}

/// The outcome as markdown.
#[must_use]
pub fn summary(o: &Outcome) -> String {
    let mut s = String::from("### rk mixed\n\n");
    let booted = o.trials.iter().filter(|t| !t.cached).count();
    let _ = writeln!(
        s,
        "Unit `{}`, {} objects differ, {} trials of which {booted} booted.\n",
        o.unit,
        o.candidates,
        o.trials.len()
    );
    if let Some(why) = &o.not_reproduced {
        let _ = writeln!(s, "Not reproduced: {why}.");
        return s;
    }
    if let Some((objects, lines)) = &o.link_failure {
        let _ = writeln!(
            s,
            "The link failed with {} rucc object(s): {}.\n",
            objects.len(),
            objects.join(", ")
        );
        for line in lines {
            let _ = writeln!(s, "    {line}");
        }
        return s;
    }
    let _ = writeln!(
        s,
        "The smallest failing set is {} object(s): {}.",
        o.objects.len(),
        o.objects
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(fuel) = &o.fuel {
        if let Some(why) = &fuel.stopped {
            let _ = writeln!(s, "\nThe fuel search stopped: {why}.");
        } else {
            let _ = writeln!(
                s,
                "\nIt passes at fuel {} and fails at {}.",
                fuel.good, fuel.bad
            );
            if let Some(d) = &fuel.divergence {
                let _ = writeln!(
                    s,
                    "The first pass that differs is `{}`, step {}, in {}.",
                    d.pass,
                    d.step,
                    if d.functions.is_empty() {
                        "no function by name".to_string()
                    } else {
                        d.functions
                            .iter()
                            .map(|f| format!("`{f}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                );
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fails_with(culprits: &[u32]) -> impl FnMut(&[u32]) -> Result<bool, ()> + '_ {
        move |set: &[u32]| Ok(culprits.iter().all(|c| set.contains(c)))
    }

    #[test]
    fn one_culprit_is_found_among_many() {
        let items: Vec<u32> = (0..3000).collect();
        let mut calls = 0;
        let mut test = fails_with(&[1234]);
        let mut counted = |set: &[u32]| {
            calls += 1;
            test(set)
        };
        assert_eq!(ddmin(&items, &mut counted), Ok(vec![1234]));
        assert!(calls < 40, "{calls} trials");
    }

    #[test]
    fn two_culprits_that_only_fail_together_are_both_kept() {
        let items: Vec<u32> = (0..64).collect();
        let mut test = fails_with(&[3, 50]);
        assert_eq!(ddmin(&items, &mut test), Ok(vec![3, 50]));
    }

    #[test]
    fn a_single_item_is_its_own_answer() {
        let mut test = fails_with(&[7]);
        assert_eq!(ddmin(&[7], &mut test), Ok(vec![7]));
    }

    #[test]
    fn the_fuel_search_lands_on_the_first_failing_step() {
        let mut calls = 0;
        let mut fails = |n: u32| -> Result<bool, ()> {
            calls += 1;
            Ok(n >= 4321)
        };
        assert_eq!(bisect_fuel(0, u32::MAX, &mut fails), Ok(4321));
        assert!(calls <= 32);
    }

    #[test]
    fn a_dump_name_gives_its_step_and_pass() {
        assert_eq!(
            dump_step("fork.c.07-after-sroa.ir"),
            Some((7, "sroa".to_string()))
        );
        assert_eq!(
            dump_step("a.b.c.12-after-simplify-cfg.ir"),
            Some((12, "simplify-cfg".to_string()))
        );
        assert_eq!(dump_step("fork.c.07-before-sroa.ir"), None);
        assert_eq!(dump_step("notes.txt"), None);
    }

    #[test]
    fn the_first_differing_pass_names_the_function_it_changed() {
        let f = "func @f(i32) -> i32, linkage(external) {\nblock0(%0: i32):\n    return %0\n}\n";
        let g = "func @g() {\nblock0:\n    return\n}\n";
        let g2 = "func @g() {\nblock0:\n    %1 = iconst.i32 1\n    return\n}\n";
        let good: BTreeMap<String, String> = [
            ("x.c.00-after-fold.ir".to_string(), format!("{f}{g}")),
            ("x.c.01-after-sroa.ir".to_string(), format!("{f}{g}")),
            ("x.c.02-after-gvn.ir".to_string(), format!("{f}{g}")),
            ("x.c.01-before-sroa.ir".to_string(), "ignored".to_string()),
        ]
        .into();
        let mut bad = good.clone();
        bad.insert("x.c.01-after-sroa.ir".to_string(), format!("{f}{g2}"));
        bad.insert("x.c.02-after-gvn.ir".to_string(), g2.to_string());
        let d = first_divergence(&good, &bad).expect("they differ");
        assert_eq!(d.pass, "sroa");
        assert_eq!(d.step, 1);
        assert_eq!(d.functions, ["g"]);
        assert_eq!(first_divergence(&good, &good), None);
    }

    #[test]
    fn a_recompile_writes_elsewhere_and_leaves_the_dependency_file_out() {
        let record: CompileRecord = serde_json::from_value(serde_json::json!({
            "started": 0.0,
            "argv": [
                "rk-cc",
                "-Wp,-MMD,kernel/.fork.o.d",
                "-O2",
                "-c",
                "-o",
                "kernel/fork.o",
                "/src/kernel/fork.c",
            ],
            "compiler": "rucc",
            "cwd": "/out",
            "wall-seconds": 0.0,
        }))
        .expect("a record");
        let args = recompile_args(
            &record,
            Path::new("/run/fuel-3.o"),
            &["-fpass-fuel-global=3".to_string()],
        );
        assert_eq!(
            args,
            [
                "-O2",
                "-c",
                "/src/kernel/fork.c",
                "-o",
                "/run/fuel-3.o",
                "-fpass-fuel-global=3"
            ]
        );
        assert_eq!(output_of(&record.argv).as_deref(), Some("kernel/fork.o"));
        assert_eq!(
            output_of(&["cc".to_string(), "-ox.o".to_string()]).as_deref(),
            Some("x.o")
        );
    }

    #[test]
    fn a_unit_is_graded_by_the_suite_that_reports_it() {
        assert_eq!(suite_of("boot"), "smoke");
        assert_eq!(suite_of("smoke:proc"), "smoke");
        assert_eq!(suite_of("kunit:list.list_add"), "kunit");
        assert_eq!(suite_of("kunit-module:lib/test_list.ko"), "kunit");
        assert_eq!(
            suite_of("kselftest:timers:posix_timers"),
            "kselftest:timers"
        );
        assert_eq!(suite_of("ltp:syscalls:abort01"), "ltp:syscalls");
    }

    #[test]
    fn a_summary_names_the_object_and_the_pass() {
        let o = Outcome {
            reference: PathBuf::from("/r"),
            other: PathBuf::from("/o"),
            unit: "smoke:proc".to_string(),
            candidates: 900,
            objects: vec!["fs/proc/base.o".to_string()],
            fuel: Some(Fuel {
                good: 41,
                bad: 42,
                divergence: Some(Divergence {
                    pass: "gvn".to_string(),
                    step: 9,
                    functions: vec!["proc_pid_lookup".to_string()],
                }),
                stopped: None,
            }),
            link_failure: None,
            not_reproduced: None,
            trials: Vec::new(),
        };
        let s = summary(&o);
        assert!(s.contains("`fs/proc/base.o`"), "{s}");
        assert!(s.contains("fails at 42"), "{s}");
        assert!(s.contains("`gvn`, step 9, in `proc_pid_lookup`"), "{s}");
        assert!(o.found());
    }
}
