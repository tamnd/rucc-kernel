//! `sets.toml` and `rk sets`: the rules that decide which trees are pinned.
//!
//! The plan's sets (04.1) are rules, not lists. `current` is every line kernel.org still
//! maintains, so its members change every week. `rk sets` reads `releases.json`, applies the
//! rules, looks up each tarball's hash, and prints how the result differs from `pins.toml`. With
//! `--write` it replaces `pins.toml`, which then goes through review like any other change.

use crate::kernelorg::{self, Release};
use crate::pins::{Pin, Pins};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Sets {
    /// Which moniker's release becomes the default pin.
    pub default_moniker: String,
    /// Every set.
    #[serde(rename = "set")]
    pub sets: Vec<Set>,
}

/// One set rule.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Set {
    /// The name, as in `current`.
    pub name: String,
    /// The kernel.org monikers the set takes, as in `stable` and `longterm`.
    pub monikers: Vec<String>,
    /// Whether lines kernel.org has marked end of life are kept.
    #[serde(default)]
    pub include_eol: bool,
}

impl Sets {
    /// Read `sets.toml`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The releases each set takes, as version to (moniker, sets), in the order kernel.org lists.
    #[must_use]
    pub fn expand(&self, releases: &[Release]) -> Vec<(String, String, Vec<String>)> {
        let mut out: Vec<(String, String, Vec<String>)> = Vec::new();
        for release in releases {
            let sets: Vec<String> = self
                .sets
                .iter()
                .filter(|set| {
                    set.monikers.contains(&release.moniker) && (set.include_eol || !release.iseol)
                })
                .map(|set| set.name.clone())
                .collect();
            if !sets.is_empty() {
                out.push((release.version.clone(), release.moniker.clone(), sets));
            }
        }
        out
    }

    /// The default pin among the expanded releases: the first with the default moniker.
    #[must_use]
    pub fn default_of(&self, expanded: &[(String, String, Vec<String>)]) -> Option<String> {
        expanded
            .iter()
            .find(|(_, moniker, _)| *moniker == self.default_moniker)
            .map(|(version, _, _)| version.clone())
    }
}

/// A file name to SHA-256 map, as `sha256sums.asc` gives.
pub type Sums = BTreeMap<String, String>;

/// Make the pins for the expanded sets, looking hashes up with `sums_for`, which is given a
/// version and returns the parsed `sha256sums.asc` of its directory.
pub fn pins_for(
    sets: &Sets,
    releases: &[Release],
    sums_for: &mut dyn FnMut(&str) -> Result<Sums, String>,
) -> Result<Pins, String> {
    let expanded = sets.expand(releases);
    let default = sets
        .default_of(&expanded)
        .ok_or_else(|| format!("no release has the moniker {}", sets.default_moniker))?;
    let mut pins = Vec::new();
    for (version, moniker, in_sets) in expanded {
        let name = format!("linux-{version}.tar.xz");
        let sums = sums_for(&version)?;
        let sha256 = sums
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("sha256sums.asc does not list {name}"))?;
        pins.push(Pin {
            url: kernelorg::tarball_url(&version),
            version,
            moniker,
            sets: in_sets,
            sha256,
        });
    }
    Ok(Pins { default, pins })
}

/// The lines that describe how `new` differs from `old`, one per changed pin.
#[must_use]
pub fn describe(old: Option<&Pins>, new: &Pins) -> Vec<String> {
    let mut lines = Vec::new();
    let empty = Vec::new();
    let old_pins = old.map_or(&empty, |o| &o.pins);
    for pin in &new.pins {
        match old_pins.iter().find(|o| o.version == pin.version) {
            None => lines.push(format!(
                "+ {} ({}, {})",
                pin.version,
                pin.moniker,
                pin.sets.join(",")
            )),
            Some(o) if o != pin => lines.push(format!("~ {} changed", pin.version)),
            Some(_) => {}
        }
    }
    for pin in old_pins {
        if !new.pins.iter().any(|n| n.version == pin.version) {
            lines.push(format!("- {}", pin.version));
        }
    }
    if let Some(o) = old
        && o.default != new.default
    {
        lines.push(format!("default {} -> {}", o.default, new.default));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETS: &str = r#"
default-moniker = "stable"

[[set]]
name = "current"
monikers = ["stable", "longterm"]
"#;

    fn releases() -> Vec<Release> {
        kernelorg::parse_releases(
            r#"{"releases":[
            {"moniker":"mainline","version":"7.3-rc5","iseol":false},
            {"moniker":"stable","version":"7.2.8","iseol":false},
            {"moniker":"stable","version":"7.1.13","iseol":true},
            {"moniker":"longterm","version":"6.12.111","iseol":false}]}"#,
        )
        .unwrap()
    }

    #[allow(clippy::unnecessary_wraps)]
    fn sums(version: &str) -> Result<Sums, String> {
        Ok([(format!("linux-{version}.tar.xz"), "a".repeat(64))].into())
    }

    #[test]
    fn current_is_the_maintained_stable_and_longterm_lines() {
        let sets: Sets = toml::from_str(SETS).unwrap();
        let pins = pins_for(&sets, &releases(), &mut sums).unwrap();
        let versions: Vec<&str> = pins.pins.iter().map(|p| p.version.as_str()).collect();
        assert_eq!(versions, ["7.2.8", "6.12.111"]);
        assert_eq!(pins.default, "7.2.8");
    }

    #[test]
    fn a_new_point_release_shows_as_one_added_and_one_removed() {
        let sets: Sets = toml::from_str(SETS).unwrap();
        let old = pins_for(&sets, &releases(), &mut sums).unwrap();
        let mut newer = releases();
        newer[1].version = "7.2.9".into();
        let new = pins_for(&sets, &newer, &mut sums).unwrap();
        let lines = describe(Some(&old), &new);
        assert!(lines.contains(&"+ 7.2.9 (stable, current)".to_string()));
        assert!(lines.contains(&"- 7.2.8".to_string()));
        assert!(lines.contains(&"default 7.2.8 -> 7.2.9".to_string()));
    }
}
