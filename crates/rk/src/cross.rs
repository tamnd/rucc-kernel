//! `rk cross-modules`: rucc modules in the GCC kernel and GCC modules in the rucc kernel (plan
//! 9.7).
//!
//! A module only loads into a kernel when its `vermagic` is the kernel's, every symbol it imports
//! is exported, and the CRC it recorded for each one in `__versions` is the CRC the kernel has.
//! Two builds of the same tree and config agree on all of that only when the two compilers left
//! the same types behind for genksyms and the same layout behind for the structures a module
//! touches. So this is the module boundary's proof of ABI compatibility, the way the compat
//! corpus is for user programs.
//!
//! First every module of each build is checked against the other kernel without booting:
//! relocations, the whole `vermagic`, symbols the kernel does not export and CRCs that differ.
//! Then three boots run the kunit suite, the reference kernel with its own modules, the reference
//! kernel with the rucc modules, and the rucc kernel with the reference modules. The units the
//! first passes in every run are graded, which is how the modules that load in QEMU without
//! hardware are found, and both crossed boots must pass every one of them. A splat in a crossed
//! boot that the reference never printed fails the run too.
//!
//! What comes out is `cross-modules.json` and `summary.md` in the run directory, next to the
//! console of every boot.

use crate::boot;
use crate::modules::{self, Module};
use crate::personas::Row;
use crate::symvers::Export;
use crate::tap::Status;
use crate::testrun::{self, Kind, Results, Units};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Everything `rk cross-modules` was asked to do.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The reference build directory.
    pub reference: PathBuf,
    /// The rucc build directory.
    pub other: PathBuf,
    /// The row both were built for.
    pub row: Row,
    /// The userland, a static busybox.
    pub busybox: Vec<u8>,
    /// How many times each boot runs.
    pub runs: usize,
    /// Seconds each boot may take.
    pub timeout: u64,
    /// The run directory.
    pub out: PathBuf,
}

/// The three boots: which kernel, whose modules, and the name in the run directory.
const CELLS: [(Side, Side, &str); 3] = [
    (Side::Reference, Side::Reference, "reference"),
    (
        Side::Reference,
        Side::Other,
        "reference-kernel-rucc-modules",
    ),
    (
        Side::Other,
        Side::Reference,
        "rucc-kernel-reference-modules",
    ),
];

/// One of the two builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Reference,
    Other,
}

impl Plan {
    fn dir(&self, side: Side) -> &Path {
        match side {
            Side::Reference => &self.reference,
            Side::Other => &self.other,
        }
    }
}

/// What is wrong with loading a module into a kernel whose modules carry `vermagic` and whose
/// exports are `symvers`, empty when nothing is. On top of `rk modules-audit`'s checks this
/// compares the whole `vermagic` and not only the release, and names the imports the kernel does
/// not export at all.
#[must_use]
pub fn problems(m: &Module, vermagic: &str, symvers: &BTreeMap<String, Export>) -> Vec<String> {
    let release = vermagic.split(' ').next().unwrap_or_default();
    let mut out = modules::problems(m, release, symvers);
    if !m.audited {
        return out;
    }
    if !m.vermagic.is_empty()
        && !vermagic.is_empty()
        && m.vermagic.split(' ').next() == Some(release)
        && m.vermagic.trim() != vermagic.trim()
    {
        out.push(format!(
            "vermagic \"{}\" is not the kernel's \"{}\"",
            m.vermagic.trim(),
            vermagic.trim()
        ));
    }
    if !symvers.is_empty() {
        let missing: Vec<&str> = m
            .versions
            .keys()
            .filter(|s| !symvers.contains_key(*s))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            out.push(format!(
                "imports {} the kernel does not export",
                missing.join(", ")
            ));
        }
    }
    out
}

/// The modules of a build by the name in `modules.order`, read.
fn read_modules(build: &Path) -> Result<BTreeMap<String, Module>, String> {
    let mut out = BTreeMap::new();
    for (name, bytes) in testrun::modules(build)? {
        let Some(name) = name.strip_prefix("lib/modules/rk/") else {
            continue;
        };
        if name == "order" {
            continue;
        }
        let m = modules::read(&bytes).map_err(|e| format!("{name}: {e}"))?;
        out.insert(name.to_string(), m);
    }
    Ok(out)
}

/// The `vermagic` a kernel's own modules carry, which is the one it wants: the most common one,
/// so that a module with a broken `.modinfo` does not decide it.
#[must_use]
pub fn kernel_vermagic(own: &BTreeMap<String, Module>) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for m in own.values().filter(|m| !m.vermagic.is_empty()) {
        *counts.entry(m.vermagic.trim()).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(v, _)| v.to_string())
        .unwrap_or_default()
}

/// Every module of `modules` against the kernel of `kernel`, with its problems, leaving out the
/// ones that have none.
fn check(
    modules: &BTreeMap<String, Module>,
    kernel: &BTreeMap<String, Module>,
    kernel_dir: &Path,
) -> BTreeMap<String, Vec<String>> {
    let vermagic = kernel_vermagic(kernel);
    let (symvers, _) = crate::symvers::load(kernel_dir);
    modules
        .iter()
        .map(|(name, m)| (name.clone(), problems(m, &vermagic, &symvers)))
        .filter(|(_, p)| !p.is_empty())
        .collect()
}

/// A unit in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Unit {
    /// Its name.
    pub name: String,
    /// The reference kernel with its own modules, each run.
    pub reference: Vec<Status>,
    /// The reference kernel with the rucc modules.
    pub rucc_modules: Vec<Status>,
    /// The rucc kernel with the reference modules.
    pub rucc_kernel: Vec<Status>,
    /// Whether it is graded: the reference passed it every time.
    pub graded: bool,
    /// Whether both crossed boots passed it every time.
    pub passed: bool,
}

/// Grade the two crossed boots against the reference's own over `runs` runs.
#[must_use]
pub fn grade(cells: &[Results; 3], runs: usize) -> Vec<Unit> {
    let names: BTreeSet<&String> = cells.iter().flat_map(BTreeMap::keys).collect();
    let missing = vec![Status::Missing; runs];
    let all = |list: &[Status]| list.len() == runs && list.iter().all(|s| *s == Status::Pass);
    names
        .into_iter()
        .map(|name| {
            let [r, m, k] = cells
                .each_ref()
                .map(|c| c.get(name).unwrap_or(&missing).clone());
            Unit {
                name: name.clone(),
                graded: all(&r),
                passed: all(&m) && all(&k),
                reference: r,
                rucc_modules: m,
                rucc_kernel: k,
            }
        })
        .collect()
}

/// What `rk cross-modules` found, written to `cross-modules.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// The reference build directory.
    pub reference: PathBuf,
    /// The rucc build directory.
    pub other: PathBuf,
    /// Runs of each boot.
    pub runs: usize,
    /// The rucc modules that cannot load into the reference kernel, with why.
    pub rucc_modules_refused: BTreeMap<String, Vec<String>>,
    /// The reference modules that cannot load into the rucc kernel, with why.
    pub reference_modules_refused: BTreeMap<String, Vec<String>>,
    /// Every unit any boot reported.
    pub units: Vec<Unit>,
    /// Splats in a crossed boot that the reference's own boot never printed.
    pub splats: BTreeSet<String>,
}

impl Outcome {
    /// The graded units a crossed boot failed.
    #[must_use]
    pub fn failed(&self) -> Vec<&Unit> {
        self.units
            .iter()
            .filter(|u| u.graded && !u.passed)
            .collect()
    }

    /// Whether every module could load both ways, every graded unit passed and no new splat
    /// came out.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.rucc_modules_refused.is_empty()
            && self.reference_modules_refused.is_empty()
            && self.failed().is_empty()
            && self.splats.is_empty()
    }
}

/// One kunit boot of `kernel` with the modules of `modules`.
fn boot_cell(
    plan: &Plan,
    kernel: &Path,
    modules: &Path,
    stem: &str,
) -> Result<(Units, BTreeSet<String>), String> {
    let files = testrun::modules(modules)?;
    let initramfs = plan.out.join(format!("{stem}.cpio"));
    std::fs::write(&initramfs, boot::initramfs_with(&plan.busybox, &files))
        .map_err(|e| format!("writing {}: {e}", initramfs.display()))?;
    eprintln!("rk: {stem}");
    let outcome = boot::run(&boot::Plan {
        build: kernel.to_path_buf(),
        row: plan.row.clone(),
        initramfs: initramfs.clone(),
        timeout: plan.timeout,
        append: "rk.suite=kunit".to_string(),
        stem: Some(plan.out.join(stem)),
    });
    let _ = std::fs::remove_file(&initramfs);
    let outcome = outcome?;
    let log = plan.out.join(format!("{stem}.log"));
    let console =
        std::fs::read_to_string(&log).map_err(|e| format!("reading {}: {e}", log.display()))?;
    Ok((
        testrun::units_of("kunit", &[Kind::Boot, Kind::Kunit], &outcome, &console),
        crate::dmesg::splats(&console),
    ))
}

/// Check both ways, boot the three cells, grade, and write `cross-modules.json` and
/// `summary.md`.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    std::fs::create_dir_all(&plan.out)
        .map_err(|e| format!("creating {}: {e}", plan.out.display()))?;
    let reference_modules = read_modules(&plan.reference)?;
    let other_modules = read_modules(&plan.other)?;
    if reference_modules.is_empty() || other_modules.is_empty() {
        return Err(
            "both builds need modules; build with --targets \"<image> modules\" and a config that makes some"
                .to_string(),
        );
    }
    let rucc_modules_refused = check(&other_modules, &reference_modules, &plan.reference);
    let reference_modules_refused = check(&reference_modules, &other_modules, &plan.other);

    let mut cells: [Results; 3] = Default::default();
    let mut own = BTreeSet::new();
    let mut crossed = BTreeSet::new();
    for run in 0..plan.runs {
        for (i, (kernel, modules, name)) in CELLS.iter().enumerate() {
            let (units, found) = boot_cell(
                plan,
                plan.dir(*kernel),
                plan.dir(*modules),
                &format!("{name}-{}", run + 1),
            )?;
            testrun::add_run(&mut cells[i], run, &units);
            if i == 0 {
                own.extend(found);
            } else {
                crossed.extend(found);
            }
        }
    }
    let outcome = Outcome {
        reference: plan.reference.clone(),
        other: plan.other.clone(),
        runs: plan.runs,
        rucc_modules_refused,
        reference_modules_refused,
        units: grade(&cells, plan.runs),
        splats: crossed.difference(&own).cloned().collect(),
    };
    let json = serde_json::to_string_pretty(&outcome).unwrap_or_default();
    std::fs::write(plan.out.join("cross-modules.json"), json + "\n")
        .map_err(|e| format!("writing cross-modules.json: {e}"))?;
    std::fs::write(plan.out.join("summary.md"), summary(&outcome))
        .map_err(|e| format!("writing summary.md: {e}"))?;
    Ok(outcome)
}

/// A unit's results over the runs, as words.
fn statuses(list: &[Status]) -> String {
    list.iter().map(|s| s.word()).collect::<Vec<_>>().join(" ")
}

/// A list of refused modules as markdown.
fn refused(s: &mut String, title: &str, list: &BTreeMap<String, Vec<String>>) {
    if list.is_empty() {
        return;
    }
    let _ = writeln!(s, "{title}:\n");
    for (name, problems) in list.iter().take(100) {
        let _ = writeln!(s, "- `{name}`: {}", problems.join("; "));
    }
    if list.len() > 100 {
        let _ = writeln!(s, "\nand {} more.", list.len() - 100);
    }
    s.push('\n');
}

/// The outcome as markdown.
#[must_use]
pub fn summary(o: &Outcome) -> String {
    let mut s = String::from("### rk cross-modules\n\n");
    let graded = o.units.iter().filter(|u| u.graded).count();
    let failed = o.failed();
    let _ = writeln!(
        s,
        "Over {} runs the reference kernel passed {graded} units every time with its own modules, and {} of them passed both ways across. {} rucc modules cannot load into the reference kernel and {} reference modules cannot load into the rucc kernel. {}.\n",
        o.runs,
        graded - failed.len(),
        o.rucc_modules_refused.len(),
        o.reference_modules_refused.len(),
        if o.passed() { "Passed" } else { "Failed" }
    );
    refused(
        &mut s,
        "rucc modules the reference kernel refuses",
        &o.rucc_modules_refused,
    );
    refused(
        &mut s,
        "Reference modules the rucc kernel refuses",
        &o.reference_modules_refused,
    );
    if !failed.is_empty() {
        s.push_str("| unit | reference | rucc modules | rucc kernel |\n|---|---|---|---|\n");
        for u in failed.iter().take(200) {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                u.name.replace('|', "\\|"),
                statuses(&u.reference),
                statuses(&u.rucc_modules),
                statuses(&u.rucc_kernel),
            );
        }
        if failed.len() > 200 {
            let _ = writeln!(s, "\nand {} more.", failed.len() - 200);
        }
        s.push('\n');
    }
    if !o.splats.is_empty() {
        s.push_str("Splats only in a crossed boot:\n\n");
        for splat in &o.splats {
            let _ = writeln!(s, "- `{}`", splat.replace('`', "'"));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::elf::{R_X86_64_PC32, R_X86_64_REX_GOTPCRELX};

    fn export(crc: &str) -> Export {
        Export {
            crc: crc.to_string(),
            module: "vmlinux".to_string(),
            kind: "EXPORT_SYMBOL".to_string(),
            namespace: String::new(),
        }
    }

    fn module(vermagic: &str, versions: &[(&str, u64)]) -> Module {
        Module {
            audited: true,
            relocs: BTreeMap::from([(R_X86_64_PC32, 4)]),
            vermagic: vermagic.to_string(),
            versions: versions
                .iter()
                .map(|(s, c)| ((*s).to_string(), *c))
                .collect(),
        }
    }

    const MAGIC: &str = "7.2.8 SMP preempt mod_unload ";

    #[test]
    fn a_module_that_matches_the_kernel_has_no_problems() {
        let symvers = BTreeMap::from([("printk".to_string(), export("0x12345678"))]);
        let m = module(MAGIC, &[("printk", 0x1234_5678)]);
        assert!(problems(&m, MAGIC.trim(), &symvers).is_empty());
    }

    #[test]
    fn every_way_a_module_is_refused_is_named() {
        let symvers = BTreeMap::from([("printk".to_string(), export("0x12345678"))]);
        let mut m = module(
            "7.2.8 SMP mod_unload ",
            &[("printk", 0x8765_4321), ("gone", 1)],
        );
        m.relocs.insert(R_X86_64_REX_GOTPCRELX, 2);
        let p = problems(&m, MAGIC, &symvers);
        assert!(p.iter().any(|p| p.contains("REX_GOTPCRELX")), "{p:?}");
        assert!(p.iter().any(|p| p.contains("is not the kernel's")), "{p:?}");
        assert!(
            p.iter().any(|p| p.contains("printk imported with CRC")),
            "{p:?}"
        );
        assert!(p.iter().any(|p| p.contains("imports gone")), "{p:?}");
        // A different release is said once, by the audit's own check.
        let p = problems(&module("6.12.50 SMP ", &[]), MAGIC, &symvers);
        assert_eq!(p.len(), 1, "{p:?}");
    }

    #[test]
    fn the_kernel_wants_what_most_of_its_modules_carry() {
        let own = BTreeMap::from([
            ("a.ko".to_string(), module(MAGIC, &[])),
            ("b.ko".to_string(), module(MAGIC, &[])),
            ("c.ko".to_string(), module("junk", &[])),
            ("d.ko".to_string(), module("", &[])),
        ]);
        assert_eq!(kernel_vermagic(&own), MAGIC.trim());
        assert_eq!(kernel_vermagic(&BTreeMap::new()), "");
    }

    #[test]
    fn a_unit_passes_only_when_both_crossed_boots_pass_it() {
        let cell = |pairs: &[(&str, Status)]| -> Results {
            let mut r = Results::new();
            let units: Units = pairs.iter().map(|(u, s)| ((*u).to_string(), *s)).collect();
            testrun::add_run(&mut r, 0, &units);
            r
        };
        let cells = [
            cell(&[
                ("boot", Status::Pass),
                ("kunit:list.a", Status::Pass),
                ("kunit-module:lib/hw.ko", Status::Fail),
            ]),
            cell(&[("boot", Status::Pass), ("kunit:list.a", Status::Pass)]),
            cell(&[
                ("boot", Status::Pass),
                ("kunit:list.a", Status::Fail),
                ("kunit-module:lib/hw.ko", Status::Fail),
            ]),
        ];
        let units = grade(&cells, 1);
        let by: BTreeMap<&str, &Unit> = units.iter().map(|u| (u.name.as_str(), u)).collect();
        assert!(by["boot"].graded && by["boot"].passed);
        assert!(by["kunit:list.a"].graded && !by["kunit:list.a"].passed);
        assert!(!by["kunit-module:lib/hw.ko"].graded);
        assert_eq!(by["kunit-module:lib/hw.ko"].rucc_modules, [Status::Missing]);
        let o = Outcome {
            reference: PathBuf::new(),
            other: PathBuf::new(),
            runs: 1,
            rucc_modules_refused: BTreeMap::new(),
            reference_modules_refused: BTreeMap::from([(
                "lib/x.ko".to_string(),
                vec!["imports gone the kernel does not export".to_string()],
            )]),
            units,
            splats: BTreeSet::new(),
        };
        assert!(!o.passed());
        let text = summary(&o);
        assert!(
            text.contains("| kunit:list.a | pass | pass | fail |"),
            "{text}"
        );
        assert!(text.contains("- `lib/x.ko`: imports gone"), "{text}");
    }
}
