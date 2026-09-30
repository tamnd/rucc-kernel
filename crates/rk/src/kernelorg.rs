//! What kernel.org publishes: the list of maintained releases and the SHA-256 lists.
//!
//! `releases.json` names the mainline rc, the stable line and every longterm line with its latest
//! point release. `sha256sums.asc` in each major directory lists the hash of every tarball there.
//! The list is signed, and the signature is checked by `gpg` when a machine has the kernel.org
//! keys; the hash in `pins.toml` is what every fetch is held to either way.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Where `releases.json` lives.
pub const RELEASES_URL: &str = "https://www.kernel.org/releases.json";

/// The base of every tarball URL.
pub const CDN: &str = "https://cdn.kernel.org/pub/linux/kernel";

/// One entry of `releases.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    /// `mainline`, `stable`, `longterm` or `linux-next`.
    pub moniker: String,
    /// The version, as in `7.2.8` or `7.3-rc5`.
    pub version: String,
    /// Whether kernel.org has marked the line end of life.
    #[serde(default)]
    pub iseol: bool,
}

#[derive(Deserialize)]
struct ReleasesFile {
    releases: Vec<Release>,
}

/// Parse the text of `releases.json`.
pub fn parse_releases(text: &str) -> Result<Vec<Release>, String> {
    let file: ReleasesFile =
        serde_json::from_str(text).map_err(|e| format!("releases.json: {e}"))?;
    Ok(file.releases)
}

/// The directory a version's tarball is in, as in `v7.x` or `v2.6`.
#[must_use]
pub fn major_dir(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or_default();
    match major {
        "1" | "2" => format!("v{major}.{}", parts.next().unwrap_or("0")),
        _ => format!("v{major}.x"),
    }
}

/// The tarball URL of a release.
#[must_use]
pub fn tarball_url(version: &str) -> String {
    format!("{CDN}/{}/linux-{version}.tar.xz", major_dir(version))
}

/// The SHA-256 list for the directory a version is in.
#[must_use]
pub fn sums_url(version: &str) -> String {
    format!("{CDN}/{}/sha256sums.asc", major_dir(version))
}

/// Parse a `sha256sums.asc`, file name to hash. The signature lines around the list do not look
/// like a hash and a name and are skipped.
#[must_use]
pub fn parse_sums(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| (name.trim().to_string(), hash.to_ascii_lowercase()))
        })
        .collect()
}

/// Download a URL to a file with curl, failing on any HTTP error.
pub fn download(url: &str, to: &Path) -> Result<(), String> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    let partial = to.with_extension("part");
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&partial)
        .arg(url)
        .status()
        .map_err(|e| format!("running curl: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("downloading {url} failed"));
    }
    std::fs::rename(&partial, to).map_err(|e| format!("moving {}: {e}", partial.display()))
}

/// Download a URL and return its text.
pub fn fetch_text(url: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "3", url])
        .output()
        .map_err(|e| format!("running curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("downloading {url} failed"));
    }
    String::from_utf8(out.stdout).map_err(|_| format!("{url} is not text"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_era_has_its_directory() {
        assert_eq!(major_dir("7.2.8"), "v7.x");
        assert_eq!(major_dir("3.16.85"), "v3.x");
        assert_eq!(major_dir("2.6.32.71"), "v2.6");
        assert_eq!(major_dir("1.0"), "v1.0");
        assert_eq!(
            tarball_url("6.1.188"),
            "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.1.188.tar.xz"
        );
    }

    #[test]
    fn the_signed_list_reads_as_a_map() {
        let text = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA256\n\n\
12e8d5a973d1ad7c5a5c69882e4022b131ed715db7003fdcd760ddf8c3e51941  linux-7.2.8.tar.xz\n\
-----BEGIN PGP SIGNATURE-----\nabc\n";
        let sums = parse_sums(text);
        assert_eq!(sums.len(), 1);
        assert!(sums["linux-7.2.8.tar.xz"].starts_with("12e8d5a9"));
    }

    #[test]
    fn releases_json_reads() {
        let text = r#"{"latest_stable":{"version":"7.2.8"},"releases":[
            {"iseol":false,"version":"7.3-rc5","moniker":"mainline","source":"x"},
            {"iseol":false,"version":"6.12.111","moniker":"longterm"}]}"#;
        let releases = parse_releases(text).unwrap();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[1].moniker, "longterm");
    }
}
