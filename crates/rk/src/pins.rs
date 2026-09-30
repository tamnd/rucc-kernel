//! `pins.toml` and `rk fetch`: kernel trees, downloaded, verified and unpacked.
//!
//! A pin is a version, a tarball URL and its SHA-256. The hash is checked every time the tarball
//! is used, including when it comes out of the cache, so a cache that somebody edited or a
//! download that was cut short can never become a build. Unless `--no-upstream-check` is given,
//! `sha256sums.asc` from kernel.org is fetched as well and has to agree with the pin.

use crate::kernelorg;
use crate::repo::cache_dir;
use rk_shim::digest::sha256_file;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pins {
    /// The pin used when none is named.
    pub default: String,
    /// Every pin.
    #[serde(rename = "pin")]
    pub pins: Vec<Pin>,
}

/// One pinned release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Pin {
    /// The release, as in `7.2.8`.
    pub version: String,
    /// What kernel.org called its line when the pin was taken: `stable` or `longterm`.
    pub moniker: String,
    /// The sets the pin belongs to.
    pub sets: Vec<String>,
    /// The tarball.
    pub url: String,
    /// Its SHA-256.
    pub sha256: String,
}

impl Pins {
    /// Read `pins.toml`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parse the text of `pins.toml`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let pins: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        for pin in &pins.pins {
            if pin.url.is_empty() || pin.sha256.len() != 64 {
                return Err(format!("pin {} needs a url and a sha256", pin.version));
            }
        }
        if pins.get(Some(&pins.default)).is_err() {
            return Err(format!("the default {} is not a pin", pins.default));
        }
        Ok(pins)
    }

    /// A pin by version, or the default.
    pub fn get(&self, version: Option<&str>) -> Result<&Pin, String> {
        let version = version.unwrap_or(&self.default);
        self.pins
            .iter()
            .find(|p| p.version == version)
            .ok_or_else(|| format!("no pin for {version} in pins.toml"))
    }

    /// Every pin in a set.
    #[must_use]
    pub fn in_set(&self, set: &str) -> Vec<&Pin> {
        self.pins
            .iter()
            .filter(|p| p.sets.iter().any(|s| s == set))
            .collect()
    }

    /// The text of the file, with the header that says where it came from.
    #[must_use]
    pub fn to_file(&self) -> String {
        let mut out = String::from(HEADER);
        out.push_str(&toml::to_string(self).unwrap_or_default());
        out
    }
}

const HEADER: &str = "\
# The kernel trees this repository builds, each a tarball on kernel.org and its SHA-256.
#
# Written by `rk sets --write` from the rules in sets.toml and kernel.org's releases.json, with the
# hashes taken from the sha256sums.asc of each directory. `rk fetch` checks the hash on every use.

";

impl Pin {
    /// The tarball in the cache.
    #[must_use]
    pub fn archive(&self) -> PathBuf {
        cache_dir()
            .join("archives")
            .join(format!("linux-{}.tar.xz", self.version))
    }

    /// The unpacked tree in the cache.
    #[must_use]
    pub fn source_dir(&self) -> PathBuf {
        cache_dir()
            .join("src")
            .join(format!("linux-{}", self.version))
    }
}

/// The marker `rk fetch` leaves in a tree it finished unpacking, holding the tarball's hash.
const MARKER: &str = ".rk-sha256";

/// Download, verify and unpack a pin, and return the tree. Work already done is not repeated.
pub fn fetch(pin: &Pin, upstream_check: bool) -> Result<PathBuf, String> {
    if upstream_check {
        let sums =
            kernelorg::parse_sums(&kernelorg::fetch_text(&kernelorg::sums_url(&pin.version))?);
        let name = format!("linux-{}.tar.xz", pin.version);
        match sums.get(&name) {
            Some(hash) if *hash == pin.sha256 => {}
            Some(hash) => {
                return Err(format!(
                    "kernel.org says {name} is {hash}, and pins.toml says {}",
                    pin.sha256
                ));
            }
            None => return Err(format!("kernel.org's sha256sums.asc does not list {name}")),
        }
    }

    let tree = pin.source_dir();
    if std::fs::read_to_string(tree.join(MARKER)).is_ok_and(|h| h.trim() == pin.sha256) {
        return Ok(tree);
    }

    let archive = pin.archive();
    if !archive.is_file() {
        kernelorg::download(&pin.url, &archive)?;
    }
    let got = sha256_file(&archive).map_err(|e| format!("hashing {}: {e}", archive.display()))?;
    if got != pin.sha256 {
        let _ = std::fs::remove_file(&archive);
        return Err(format!(
            "{} has SHA-256 {got}, and the pin says {}; the file was removed",
            archive.display(),
            pin.sha256
        ));
    }

    let parent = tree.parent().expect("the tree is under the cache");
    std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    if tree.exists() {
        std::fs::remove_dir_all(&tree).map_err(|e| format!("removing {}: {e}", tree.display()))?;
    }
    let status = Command::new("tar")
        .arg("-xJf")
        .arg(&archive)
        .arg("-C")
        .arg(parent)
        .status()
        .map_err(|e| format!("running tar: {e}"))?;
    if !status.success() || !tree.join("Makefile").is_file() {
        return Err(format!(
            "unpacking {} did not give {}",
            archive.display(),
            tree.display()
        ));
    }
    std::fs::write(tree.join(MARKER), &pin.sha256)
        .map_err(|e| format!("writing the marker in {}: {e}", tree.display()))?;
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"
default = "7.2.8"

[[pin]]
version = "7.2.8"
moniker = "stable"
sets = ["current"]
url = "https://cdn.kernel.org/pub/linux/kernel/v7.x/linux-7.2.8.tar.xz"
sha256 = "12e8d5a973d1ad7c5a5c69882e4022b131ed715db7003fdcd760ddf8c3e51941"

[[pin]]
version = "6.12.111"
moniker = "longterm"
sets = ["current"]
url = "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.12.111.tar.xz"
sha256 = "9e59dc67624188fa12a6601f9598499cd6662a9066be572b59f935e3d7849810"
"#;

    #[test]
    fn pins_read_and_are_found_by_version() {
        let pins = Pins::parse(TEXT).unwrap();
        assert_eq!(pins.get(None).unwrap().version, "7.2.8");
        assert_eq!(pins.get(Some("6.12.111")).unwrap().moniker, "longterm");
        assert!(pins.get(Some("2.6.32")).is_err());
        assert_eq!(pins.in_set("current").len(), 2);
    }

    #[test]
    fn a_default_that_is_not_pinned_is_refused() {
        let text = TEXT.replace("default = \"7.2.8\"", "default = \"9.9\"");
        assert!(Pins::parse(&text).unwrap_err().contains("9.9"));
    }

    #[test]
    fn the_written_file_reads_back() {
        let pins = Pins::parse(TEXT).unwrap();
        assert_eq!(Pins::parse(&pins.to_file()).unwrap(), pins);
    }
}
