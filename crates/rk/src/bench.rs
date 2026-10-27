//! `rk bench`: the run time of a rucc kernel against the gcc kernel of the same tree (12.2).
//!
//! Both kernels boot the same initramfs, with `rk-init/bench.c` built by the host gcc as `/bench`
//! and the `bench` suite of rk-init. `/bench` prints one `RK-BENCH <name> <ns>` line for each
//! benchmark, or `RK-BENCH-SKIP <name>` when the kernel lacks what it needs. The kernels boot
//! one after the other, never side by side, and in turn: rucc, gcc, rucc and so on, so that a
//! host that gets slower during the run slows both. Each benchmark's number is the median of the
//! runs, and the ratio is the rucc median over the gcc one.
//!
//! The K8 gate of 02.5 is a geometric mean of the ratios of at most 1.02 and no ratio over 1.05,
//! where a ratio is over 1.05 when the upper end of its 95% interval is. The interval comes from
//! resampling the runs of each kernel with a fixed seed, so the same numbers give the same one.
//! `memcpy` runs no kernel code and is the control: it does not count in the mean, and when its
//! ratio is not within 5% of 1 the host was too noisy for the run to say anything.

use crate::boot;
use crate::personas::Row;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The source of `/bench`, as committed.
pub const SOURCE: &str = include_str!("../../../rk-init/bench.c");

/// The benchmark that runs only user code.
pub const CONTROL: &str = "memcpy";

/// The most the geometric mean of the ratios can be.
pub const MEAN_LIMIT: f64 = 1.02;

/// The most one ratio can be, and how far the control can be from 1.
pub const ONE_LIMIT: f64 = 1.05;

/// How many times the runs are resampled for an interval.
const RESAMPLES: usize = 2000;

/// What `rk bench` does.
pub struct Plan {
    /// The gcc build.
    pub reference: PathBuf,
    /// The rucc build.
    pub other: PathBuf,
    /// The row both were built for.
    pub row: Row,
    /// A static busybox.
    pub busybox: Vec<u8>,
    /// The static `/bench` program.
    pub bench: Vec<u8>,
    /// Boots of each kernel.
    pub runs: usize,
    /// Seconds before one boot is stopped.
    pub timeout: u64,
    /// Where the consoles and `bench.json` go.
    pub out: PathBuf,
}

/// One benchmark over all the runs, in nanoseconds per operation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Bench {
    /// The gcc kernel's numbers, one for each run.
    pub reference: Vec<f64>,
    /// The rucc kernel's numbers, one for each run.
    pub other: Vec<f64>,
}

impl Bench {
    /// The rucc median over the gcc median.
    #[must_use]
    pub fn ratio(&self) -> Option<f64> {
        let (r, o) = (median(&self.reference)?, median(&self.other)?);
        (r > 0.0).then(|| o / r)
    }

    /// The 95% interval of the ratio: resample the runs of each kernel, with replacement, take the
    /// ratio of the medians each time, and keep the 2.5th and 97.5th percentiles.
    #[must_use]
    pub fn interval(&self) -> Option<(f64, f64)> {
        self.ratio()?;
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut pick = |values: &[f64]| -> Vec<f64> {
            (0..values.len())
                .map(|_| {
                    // xorshift64: enough to spread the picks, and the same every time.
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    #[allow(clippy::cast_possible_truncation)]
                    values[(seed % values.len() as u64) as usize]
                })
                .collect()
        };
        let mut ratios: Vec<f64> = (0..RESAMPLES)
            .filter_map(|_| {
                let r = median(&pick(&self.reference))?;
                let o = median(&pick(&self.other))?;
                (r > 0.0).then(|| o / r)
            })
            .collect();
        ratios.sort_by(f64::total_cmp);
        let at = |q: f64| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let i = ((ratios.len() - 1) as f64 * q).round() as usize;
            ratios[i]
        };
        Some((at(0.025), at(0.975)))
    }
}

/// What `rk bench` found, written to `bench.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// The row.
    pub row: String,
    /// `kvm` or `tcg`.
    pub accel: String,
    /// The boots of each kernel that gave numbers.
    pub runs: usize,
    /// Each benchmark by name.
    pub benches: BTreeMap<String, Bench>,
    /// The boots that failed, as `<stem>: <why>`.
    pub failed: Vec<String>,
}

impl Outcome {
    /// The geometric mean of the ratios of every benchmark but the control.
    #[must_use]
    pub fn mean(&self) -> Option<f64> {
        let ratios: Vec<f64> = self
            .benches
            .iter()
            .filter(|(name, _)| name.as_str() != CONTROL)
            .filter_map(|(_, b)| b.ratio())
            .collect();
        if ratios.is_empty() {
            return None;
        }
        let logs: f64 = ratios.iter().map(|r| r.ln()).sum();
        #[allow(clippy::cast_precision_loss)]
        Some((logs / ratios.len() as f64).exp())
    }

    /// Whether the control stayed within 5% of 1.
    #[must_use]
    pub fn quiet(&self) -> bool {
        self.benches
            .get(CONTROL)
            .and_then(Bench::ratio)
            .is_some_and(|r| (1.0 / ONE_LIMIT..=ONE_LIMIT).contains(&r))
    }

    /// Whether the run holds the K8 gate.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failed.is_empty()
            && self.quiet()
            && self.mean().is_some_and(|m| m <= MEAN_LIMIT)
            && self
                .benches
                .iter()
                .filter(|(name, _)| name.as_str() != CONTROL)
                .all(|(_, b)| b.interval().is_some_and(|(_, high)| high <= ONE_LIMIT))
    }
}

/// The median, or `None` for no numbers.
#[must_use]
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let half = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[half]
    } else {
        f64::midpoint(sorted[half - 1], sorted[half])
    })
}

/// The `RK-BENCH` lines of a console, by name.
#[must_use]
pub fn numbers(console: &str) -> BTreeMap<String, f64> {
    console
        .lines()
        .filter_map(|line| line.trim_end_matches('\r').strip_prefix("RK-BENCH "))
        .filter_map(|rest| {
            let (name, value) = rest.trim().rsplit_once(' ')?;
            Some((name.to_string(), value.parse().ok()?))
        })
        .collect()
}

/// Build `/bench` with a C compiler, static.
pub fn compile(cc: &str, out: &Path) -> Result<Vec<u8>, String> {
    std::fs::create_dir_all(out).map_err(|e| format!("creating {}: {e}", out.display()))?;
    let source = out.join("bench.c");
    let program = out.join("bench");
    std::fs::write(&source, SOURCE).map_err(|e| format!("writing {}: {e}", source.display()))?;
    let status = Command::new(cc)
        .args(["-static", "-O2", "-o"])
        .arg(&program)
        .arg(&source)
        .status()
        .map_err(|e| format!("running {cc}: {e}"))?;
    if !status.success() {
        return Err(format!("{cc} could not build {}", source.display()));
    }
    std::fs::read(&program).map_err(|e| format!("reading {}: {e}", program.display()))
}

/// Boot one kernel once and read its numbers.
fn boot_once(
    plan: &Plan,
    build: &Path,
    initramfs: &Path,
    stem: &str,
) -> Result<(String, BTreeMap<String, f64>), String> {
    eprintln!("rk: {stem}");
    let outcome = boot::run(&boot::Plan {
        build: build.to_path_buf(),
        row: plan.row.clone(),
        initramfs: initramfs.to_path_buf(),
        timeout: plan.timeout,
        append: "rk.suite=bench".to_string(),
        memory: 0,
        stem: Some(plan.out.join(stem)),
    })?;
    let log = plan.out.join(format!("{stem}.log"));
    let console =
        std::fs::read_to_string(&log).map_err(|e| format!("reading {}: {e}", log.display()))?;
    if let Some(panic) = outcome.panic {
        return Err(panic);
    }
    if outcome.timed_out || !outcome.done {
        return Err("init did not get to the end".to_string());
    }
    let numbers = numbers(&console);
    if numbers.is_empty() {
        return Err("no RK-BENCH line".to_string());
    }
    Ok((outcome.accel, numbers))
}

/// Boot both kernels in turn, `runs` times each, and write `bench.json`.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    std::fs::create_dir_all(&plan.out)
        .map_err(|e| format!("creating {}: {e}", plan.out.display()))?;
    let files = vec![("bench".to_string(), plan.bench.clone())];
    let initramfs = plan.out.join("bench.cpio");
    std::fs::write(&initramfs, boot::initramfs_with(&plan.busybox, &files))
        .map_err(|e| format!("writing {}: {e}", initramfs.display()))?;
    let mut outcome = Outcome {
        row: plan.row.name.clone(),
        ..Outcome::default()
    };
    for run in 1..=plan.runs {
        let mut pair = Vec::new();
        for (side, build) in [("rucc", &plan.other), ("gcc", &plan.reference)] {
            let stem = format!("bench-{side}-{run}");
            match boot_once(plan, build, &initramfs, &stem) {
                Ok((accel, numbers)) => {
                    outcome.accel = accel;
                    pair.push((side, numbers));
                }
                Err(why) => outcome.failed.push(format!("{stem}: {why}")),
            }
        }
        // A run counts only when both kernels gave numbers, so that every median is over the
        // same runs of the host.
        if pair.len() == 2 {
            outcome.runs += 1;
            for (side, numbers) in pair {
                for (name, value) in numbers {
                    let bench = outcome.benches.entry(name).or_default();
                    if side == "rucc" {
                        bench.other.push(value);
                    } else {
                        bench.reference.push(value);
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_file(&initramfs);
    let json = serde_json::to_string_pretty(&outcome).unwrap_or_default();
    let path = plan.out.join("bench.json");
    std::fs::write(&path, json + "\n").map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(outcome)
}

/// The run as markdown.
#[must_use]
pub fn summary(o: &Outcome) -> String {
    let mut out = format!(
        "## rk bench {} ({}, {} runs of each kernel)\n\n\
         | benchmark | gcc ns | rucc ns | rucc / gcc | 95% interval |\n\
         |---|---:|---:|---:|---|\n",
        o.row, o.accel, o.runs
    );
    for (name, b) in &o.benches {
        let cell = |v: Option<f64>| v.map_or_else(|| "-".to_string(), |v| format!("{v:.1}"));
        let ratio = b
            .ratio()
            .map_or_else(|| "-".to_string(), |r| format!("{r:.3}"));
        let interval = b.interval().map_or_else(
            || "-".to_string(),
            |(low, high)| format!("{low:.3} to {high:.3}"),
        );
        let note = if name == CONTROL { " (control)" } else { "" };
        let _ = writeln!(
            out,
            "| {name}{note} | {} | {} | {ratio} | {interval} |",
            cell(median(&b.reference)),
            cell(median(&b.other))
        );
    }
    let mean = o
        .mean()
        .map_or_else(|| "-".to_string(), |m| format!("{m:.3}"));
    let _ = writeln!(
        out,
        "\nGeometric mean of the ratios, without the control: {mean}."
    );
    if !o.quiet() {
        let _ = writeln!(
            out,
            "The control is not within 5% of 1, so the host was too noisy for this run to count."
        );
    }
    for line in &o.failed {
        let _ = writeln!(out, "- failed: {line}");
    }
    let _ = writeln!(
        out,
        "\n{}",
        if o.passed() {
            "The K8 gate holds."
        } else {
            "The K8 gate does not hold."
        }
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_numbers_are_read_from_the_console() {
        let console = "[ 1.0] booting\r\nRK-BOOTED 7.3.0 1.2\nRK-BENCH getppid 812.5\r\n\
                       RK-BENCH fork-exit 90210.0\nRK-BENCH broken x\n\
                       RK-BENCH-SKIP unix-pingpong\nRK-DONE\n";
        let got = numbers(console);
        assert_eq!(got.len(), 2);
        assert!((got["getppid"] - 812.5).abs() < 1e-9);
        assert!((got["fork-exit"] - 90210.0).abs() < 1e-9);
    }

    #[test]
    fn the_median_is_the_middle_or_the_mean_of_the_two_middles() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }

    fn bench(reference: f64, other: f64) -> Bench {
        Bench {
            reference: vec![reference],
            other: vec![other],
        }
    }

    #[test]
    fn the_interval_holds_the_ratio_and_widens_with_the_spread() {
        let tight = Bench {
            reference: vec![100.0; 11],
            other: vec![103.0; 11],
        };
        let (low, high) = tight.interval().unwrap();
        assert!((low - 1.03).abs() < 1e-9 && (high - 1.03).abs() < 1e-9);
        let loose = Bench {
            reference: vec![
                90.0, 95.0, 100.0, 105.0, 110.0, 100.0, 98.0, 102.0, 97.0, 103.0, 100.0,
            ],
            other: vec![
                93.0, 98.0, 103.0, 108.0, 113.0, 103.0, 101.0, 105.0, 100.0, 106.0, 103.0,
            ],
        };
        let (low, high) = loose.interval().unwrap();
        assert!(low < loose.ratio().unwrap() && loose.ratio().unwrap() < high);
        assert!(
            high > ONE_LIMIT,
            "a ratio of 1.03 with this spread may well be over 1.05"
        );
        assert_eq!(loose.interval(), loose.interval(), "the seed is fixed");
        assert_eq!(Bench::default().interval(), None);
    }

    #[test]
    fn the_gate_takes_the_mean_without_the_control_and_each_ratio() {
        let mut o = Outcome {
            runs: 1,
            ..Outcome::default()
        };
        o.benches.insert("getppid".into(), bench(100.0, 101.0));
        o.benches.insert("stat".into(), bench(100.0, 99.0));
        o.benches.insert(CONTROL.into(), bench(100.0, 104.0));
        assert!(o.quiet());
        assert!((o.mean().unwrap() - (1.01f64 * 0.99).sqrt()).abs() < 1e-9);
        assert!(o.passed());
        o.benches.insert("signal".into(), bench(100.0, 106.0));
        assert!(!o.passed(), "one ratio over 1.05 fails the gate");
        o.benches.remove("signal");
        o.benches.insert(CONTROL.into(), bench(100.0, 110.0));
        assert!(!o.quiet());
        assert!(!o.passed(), "a noisy host fails the run");
    }
}
