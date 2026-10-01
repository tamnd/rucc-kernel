//! Distribution kernel configurations, pinned in `configs/distro/distros.toml`.
//!
//! A distribution config is a starting `.config` rather than a kbuild target: `rk build` copies
//! the pinned file into the output directory, checks its hash, lays the overrides over it, and
//! settles it with `olddefconfig`. Each one belongs to one version and one row, the ones the
//! distribution built it for, because a config carried to another version is no longer what the
//! distribution ships.

use crate::kconfig;
use rk_shim::digest::{same_digest, sha256_bytes};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Every pinned distribution config.
#[derive(Debug, Clone, Deserialize)]
pub struct Distros {
    /// The configs, in file order.
    #[serde(rename = "distro")]
    pub distros: Vec<Distro>,
}

/// One distribution kernel config.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Distro {
    /// The name `--config` takes, as in `debian-13`.
    pub name: String,
    /// The file next to the manifest.
    pub file: String,
    /// The pinned version it belongs to.
    pub version: String,
    /// The row it belongs to.
    pub row: String,
    /// The package the config came out of.
    pub package: String,
    /// The package's SHA-256.
    pub package_sha256: String,
    /// The config's SHA-256.
    pub config_sha256: String,
    /// The compiler the distribution built with, for the record.
    pub compiler: String,
    /// `.config` lines laid over the file before `olddefconfig`.
    #[serde(default)]
    pub overrides: Vec<String>,
}

impl Distros {
    /// Read the manifest.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parse the text of the manifest.
    pub fn parse(text: &str) -> Result<Self, String> {
        let distros: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        for d in &distros.distros {
            if d.package_sha256.len() != 64 || d.config_sha256.len() != 64 {
                return Err(format!("{} needs a package and a config sha256", d.name));
            }
            if kconfig::parse_fragment(&d.overrides.join("\n")).len() != d.overrides.len() {
                return Err(format!(
                    "{} has an override that is not a config line",
                    d.name
                ));
            }
        }
        Ok(distros)
    }

    /// A config by name, if it is a distribution config at all.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Distro> {
        self.distros.iter().find(|d| d.name == name)
    }
}

impl Distro {
    /// Refuse a version or row the config was not made for.
    pub fn check(&self, version: &str, row: &str) -> Result<(), String> {
        if self.version != version {
            return Err(format!(
                "{} is the config of {}, not {version}",
                self.name, self.version
            ));
        }
        if self.row != row {
            return Err(format!(
                "{} is a config for {}, not {row}",
                self.name, self.row
            ));
        }
        Ok(())
    }

    /// The starting `.config`: the pinned file, checked against its hash, with the overrides
    /// laid over it.
    pub fn seed(&self, dir: &Path) -> Result<String, String> {
        let path: PathBuf = dir.join(&self.file);
        let bytes = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let sha256 = sha256_bytes(&bytes);
        if !same_digest(&sha256, &self.config_sha256) {
            return Err(format!(
                "{} has sha256 {sha256}, the manifest says {}",
                path.display(),
                self.config_sha256
            ));
        }
        let text =
            String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
        Ok(kconfig::merge(
            &text,
            &kconfig::parse_fragment(&self.overrides.join("\n")),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
[[distro]]
name = "test-1"
file = "test-1.config"
version = "7.2.8"
row = "X64"
package = "kernel-7.2.8.rpm"
package_sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
config_sha256 = "SUM"
compiler = "gcc 16.2.1"
overrides = ['CONFIG_SYSTEM_TRUSTED_KEYS=""', '# CONFIG_RUST is not set']
"#;

    const CONFIG: &str =
        "CONFIG_SYSTEM_TRUSTED_KEYS=\"certs/distro.pem\"\nCONFIG_RUST=y\nCONFIG_SMP=y\n";

    fn manifest() -> Distros {
        Distros::parse(&MANIFEST.replace("SUM", &sha256_bytes(CONFIG.as_bytes()))).unwrap()
    }

    #[test]
    fn a_config_seeds_with_its_overrides() {
        let dir = std::env::temp_dir().join(format!("rk-distro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test-1.config"), CONFIG).unwrap();
        let distros = manifest();
        let seeded = distros.get("test-1").unwrap().seed(&dir).unwrap();
        assert_eq!(
            seeded,
            "CONFIG_SMP=y\nCONFIG_SYSTEM_TRUSTED_KEYS=\"\"\n# CONFIG_RUST is not set\n"
        );
        std::fs::write(dir.join("test-1.config"), "CONFIG_SMP=n\n").unwrap();
        let err = distros.get("test-1").unwrap().seed(&dir).unwrap_err();
        assert!(err.contains("the manifest says"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_config_belongs_to_one_version_and_row() {
        let distros = manifest();
        let d = distros.get("test-1").unwrap();
        assert!(d.check("7.2.8", "X64").is_ok());
        assert!(
            d.check("6.12.111", "X64")
                .unwrap_err()
                .contains("not 6.12.111")
        );
        assert!(d.check("7.2.8", "A64").unwrap_err().contains("not A64"));
        assert!(distros.get("defconfig").is_none());
    }

    #[test]
    fn the_shipped_manifest_reads_and_its_files_match() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/distro");
        let distros = Distros::load(&dir.join("distros.toml")).unwrap();
        assert!(distros.get("debian-13").is_some());
        assert!(distros.get("fedora-44").is_some());
        for d in &distros.distros {
            d.seed(&dir).unwrap();
        }
    }

    #[test]
    fn an_override_must_be_a_config_line() {
        let bad = manifest_text().replace("'# CONFIG_RUST is not set'", "'RUST=n'");
        assert!(Distros::parse(&bad).unwrap_err().contains("override"));
    }

    fn manifest_text() -> String {
        MANIFEST.replace("SUM", &"1".repeat(64))
    }
}
