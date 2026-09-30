//! `rk baseline`: the reference's own results, which are what rucc is measured against.
//!
//! One version, row and configuration is built and booted with the reference several times. The
//! file written under `results/baseline` says how long each build took, whether the image came
//! out the same every time, and which smoke checks passed. A check the reference fails is not
//! held against rucc, and a build that is not reproducible with the reference cannot be compared
//! byte for byte with anything, so both are worth knowing before rucc builds a single unit.

use crate::boot;
use crate::build::{self, Compiler, Plan};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::Path;

/// One build and boot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Run {
    /// Whether the image was built.
    pub built: bool,
    /// Seconds spent configuring and building.
    pub build_seconds: f64,
    /// Units compiled.
    pub units: usize,
    /// The SHA-256 of `.config`.
    pub config_sha256: String,
    /// The SHA-256 of the image.
    pub image_sha256: String,
    /// Whether the boot passed, if it ran.
    pub booted: Option<bool>,
    /// Seconds from starting QEMU to its exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_seconds: Option<f64>,
    /// `kvm` or `tcg`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub accel: String,
    /// Smoke checks that failed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_checks: Vec<String>,
}

impl Run {
    /// The run from what `rk build` and `rk boot` found.
    #[must_use]
    pub fn from(built: &build::Outcome, booted: Option<&boot::Outcome>) -> Self {
        Self {
            built: built.built,
            build_seconds: (built.wall_seconds * 10.0).round() / 10.0,
            units: built.calls.units,
            config_sha256: built.config_sha256.clone(),
            image_sha256: built.image_sha256.clone(),
            booted: booted.map(boot::Outcome::passed),
            boot_seconds: booted.map(|b| (b.seconds * 10.0).round() / 10.0),
            accel: booted.map(|b| b.accel.clone()).unwrap_or_default(),
            failed_checks: booted
                .map(|b| {
                    b.checks
                        .iter()
                        .filter(|(_, ok)| !ok)
                        .map(|(name, _)| name.clone())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// `results/baseline/<version>-<row>-<config>-<compiler>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Baseline {
    /// The kernel version.
    pub version: String,
    /// The row.
    pub row: String,
    /// The configuration target.
    pub config: String,
    /// The fragment merged after it, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment: Option<String>,
    /// The era.
    pub era: String,
    /// The reference compiler.
    pub compiler: Compiler,
    /// Whether every run built the same image.
    pub reproducible: bool,
    /// Every run.
    pub runs: Vec<Run>,
}

impl Baseline {
    /// The baseline of a plan's runs.
    #[must_use]
    pub fn new(plan: &Plan, fragment: Option<String>, runs: Vec<Run>) -> Self {
        let reproducible = runs.first().is_some_and(|first| {
            !first.image_sha256.is_empty()
                && runs.iter().all(|r| r.image_sha256 == first.image_sha256)
        });
        Self {
            version: plan.pin.version.clone(),
            row: plan.row.name.clone(),
            config: plan.config.clone(),
            fragment,
            era: plan.era.id.clone(),
            compiler: plan.compiler.clone(),
            reproducible,
            runs,
        }
    }

    /// The file name under `results/baseline`.
    #[must_use]
    pub fn file_name(&self) -> String {
        let config = match &self.fragment {
            Some(f) => format!("{}+{f}", self.config),
            None => self.config.clone(),
        };
        format!(
            "{}-{}-{}-{}.json",
            self.version,
            self.row,
            config,
            self.compiler.label()
        )
    }

    /// Whether every run built and booted.
    #[must_use]
    pub fn passed(&self) -> bool {
        !self.runs.is_empty() && self.runs.iter().all(|r| r.built && r.booted == Some(true))
    }

    /// Write the file, making its directory.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json + "\n").map_err(|e| format!("writing {}: {e}", path.display()))
    }

    /// The baseline as markdown.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "### Baseline {} {} {} with {}\n",
            self.version,
            self.row,
            self.file_name().trim_end_matches(".json"),
            self.compiler.version
        );
        let _ = writeln!(
            s,
            "Reproducible: {}.\n",
            if self.reproducible { "yes" } else { "no" }
        );
        let _ = writeln!(
            s,
            "| run | built | build seconds | units | booted | boot seconds | failed checks |\n|---|---|---|---|---|---|---|"
        );
        for (i, r) in self.runs.iter().enumerate() {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} | {} | {} |",
                i + 1,
                if r.built { "yes" } else { "no" },
                r.build_seconds,
                r.units,
                match r.booted {
                    Some(true) => format!("yes ({})", r.accel),
                    Some(false) => format!("no ({})", r.accel),
                    None => "not run".to_string(),
                },
                r.boot_seconds.map_or_else(String::new, |t| t.to_string()),
                r.failed_checks.join(", ")
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(image: &str, booted: Option<bool>) -> Run {
        Run {
            built: !image.is_empty(),
            build_seconds: 60.0,
            units: 2000,
            config_sha256: "c".to_string(),
            image_sha256: image.to_string(),
            booted,
            boot_seconds: Some(4.0),
            accel: "kvm".to_string(),
            failed_checks: Vec::new(),
        }
    }

    fn baseline(runs: Vec<Run>) -> Baseline {
        let reproducible = runs.first().is_some_and(|first| {
            !first.image_sha256.is_empty()
                && runs.iter().all(|r| r.image_sha256 == first.image_sha256)
        });
        Baseline {
            version: "7.2.8".to_string(),
            row: "X64".to_string(),
            config: "defconfig".to_string(),
            fragment: Some("test".to_string()),
            era: "E11".to_string(),
            compiler: Compiler {
                path: "/usr/bin/gcc-14".into(),
                version: "gcc-14 (Ubuntu 14.2.0) 14.2.0".to_string(),
                sha256: String::new(),
                rucc: false,
            },
            reproducible,
            runs,
        }
    }

    #[test]
    fn the_same_image_every_time_is_reproducible() {
        let b = baseline(vec![run("a", Some(true)), run("a", Some(true))]);
        assert!(b.reproducible);
        assert!(b.passed());
        assert_eq!(b.file_name(), "7.2.8-X64-defconfig+test-gcc-14.json");
        let b = baseline(vec![run("a", Some(true)), run("b", Some(true))]);
        assert!(!b.reproducible);
    }

    #[test]
    fn a_run_that_did_not_boot_fails_the_baseline() {
        assert!(!baseline(vec![run("a", Some(true)), run("a", None)]).passed());
        assert!(
            baseline(vec![run("a", Some(false))])
                .summary()
                .contains("no (kvm)")
        );
    }
}
