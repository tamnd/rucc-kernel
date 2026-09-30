//! `personas.toml` and `rows.toml`: the eras and the rows, read and checked.
//!
//! Every kernel version falls in exactly one era, and the era says which GCC rucc claims to be
//! when it builds that version. The eras must not overlap and must leave no gap between them,
//! which `Personas::parse` checks, so that no version is ever built under a persona picked by
//! accident.

use serde::Deserialize;
use std::cmp::Ordering;
use std::path::Path;

/// `personas.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Personas {
    /// Every era, oldest first.
    #[serde(rename = "era")]
    pub eras: Vec<Era>,
}

/// One era.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Era {
    /// `E0` to `E11`.
    pub id: String,
    /// The first kernel version of the era.
    pub from: String,
    /// The last, or none for the era that is still open.
    #[serde(default)]
    pub to: Option<String>,
    /// The GCC version rucc claims, passed as `-fgnuc-version`.
    pub gnuc: String,
    /// The binutils version rucc's assembler claims.
    pub gnu_as: String,
    /// The reference toolchain.
    pub reference: Reference,
    /// The C dialect the era's kbuild asks for, or assumes.
    pub std: String,
    /// Anything worth knowing.
    #[serde(default)]
    pub notes: String,
}

/// A reference toolchain.
#[derive(Debug, Clone, Deserialize)]
pub struct Reference {
    /// The GCC release.
    pub gcc: String,
    /// The binutils release.
    pub binutils: String,
    /// The era container it lives in.
    pub container: String,
}

/// Compare two kernel versions number by number. An rc sorts before its release.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let parts = |v: &str| -> (Vec<u64>, Option<u64>) {
        let (base, rc) = match v.split_once("-rc") {
            Some((base, rc)) => (base, rc.parse().ok()),
            None => (v, None),
        };
        (
            base.split('.').map(|p| p.parse().unwrap_or(0)).collect(),
            rc,
        )
    };
    let (mut a_nums, a_rc) = parts(a);
    let (mut b_nums, b_rc) = parts(b);
    let len = a_nums.len().max(b_nums.len());
    a_nums.resize(len, 0);
    b_nums.resize(len, 0);
    a_nums.cmp(&b_nums).then(match (a_rc, b_rc) {
        (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (Some(x), Some(y)) => x.cmp(&y),
    })
}

impl Era {
    /// Whether a kernel version is in this era. The last version of an era covers its point
    /// releases, so `to = "4.1"` takes 4.1.52 as well.
    #[must_use]
    pub fn contains(&self, version: &str) -> bool {
        if compare_versions(version, &self.from) == Ordering::Less {
            return false;
        }
        let Some(to) = &self.to else { return true };
        let depth = to.split('.').count();
        let head: Vec<&str> = version
            .split('-')
            .next()
            .unwrap_or(version)
            .split('.')
            .collect();
        let cut = head[..depth.min(head.len())].join(".");
        compare_versions(&cut, to) != Ordering::Greater
    }
}

impl Personas {
    /// Read `personas.toml`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parse and check that only the last era is open.
    pub fn parse(text: &str) -> Result<Self, String> {
        let personas: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        let last = personas.eras.len().saturating_sub(1);
        for (i, era) in personas.eras.iter().enumerate() {
            if era.to.is_none() && i != last {
                return Err(format!("{} is open but is not the last era", era.id));
            }
        }
        Ok(personas)
    }

    /// The one era a version is in.
    pub fn era_for(&self, version: &str) -> Result<&Era, String> {
        let found: Vec<&Era> = self.eras.iter().filter(|e| e.contains(version)).collect();
        match found.as_slice() {
            [era] => Ok(era),
            [] => Err(format!("{version} is in no era of personas.toml")),
            many => Err(format!(
                "{version} is in {} eras: {}",
                many.len(),
                many.iter()
                    .map(|e| e.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

/// `rows.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Rows {
    /// Every row.
    #[serde(rename = "row")]
    pub rows: Vec<Row>,
}

/// One row.
#[derive(Debug, Clone, Deserialize)]
pub struct Row {
    /// `X64`, `A64`, `X32` or `R64`.
    pub name: String,
    /// kbuild's `ARCH`.
    pub arch: String,
    /// The `CROSS_COMPILE` prefix, empty on the row's own architecture.
    pub cross: String,
    /// The make target of the boot image.
    pub image: String,
    /// The QEMU binary.
    pub qemu: String,
    /// The QEMU machine.
    pub machine: String,
    /// The QEMU CPU.
    pub cpu: String,
    /// The serial console device name.
    pub console: String,
}

impl Rows {
    /// Read `rows.toml`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A row by name.
    pub fn get(&self, name: &str) -> Result<&Row, String> {
        self.rows
            .iter()
            .find(|r| r.name == name)
            .ok_or_else(|| format!("no row {name} in rows.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> Personas {
        Personas::parse(include_str!("../../../personas.toml")).unwrap()
    }

    #[test]
    fn versions_compare_by_number_and_rc_comes_first() {
        assert_eq!(compare_versions("6.12.111", "6.6.157"), Ordering::Greater);
        assert_eq!(compare_versions("2.6.32.71", "2.6.39"), Ordering::Less);
        assert_eq!(compare_versions("7.3-rc5", "7.3"), Ordering::Less);
        assert_eq!(compare_versions("7.3", "7.3.0"), Ordering::Equal);
    }

    #[test]
    fn every_era_boundary_lands_where_the_plan_says() {
        let p = committed();
        let era = |v: &str| p.era_for(v).unwrap().id.clone();
        assert_eq!(era("1.0"), "E0");
        assert_eq!(era("2.4.37.11"), "E2");
        assert_eq!(era("2.6.12"), "E3");
        assert_eq!(era("2.6.39.4"), "E4");
        assert_eq!(era("3.17.8"), "E5");
        assert_eq!(era("4.1.52"), "E6");
        assert_eq!(era("4.4.302"), "E7");
        assert_eq!(era("5.10.270"), "E8");
        assert_eq!(era("5.15.221"), "E9");
        assert_eq!(era("6.1.188"), "E10");
        assert_eq!(era("6.14.11"), "E10");
        assert_eq!(era("6.15"), "E11");
        assert_eq!(era("7.3-rc5"), "E11");
    }

    #[test]
    fn every_committed_pin_has_exactly_one_era() {
        let pins = crate::pins::Pins::parse(include_str!("../../../pins.toml")).unwrap();
        let p = committed();
        for pin in &pins.pins {
            p.era_for(&pin.version).unwrap();
        }
    }

    #[test]
    fn the_committed_rows_read() {
        let rows: Rows = toml::from_str(include_str!("../../../rows.toml")).unwrap();
        assert_eq!(rows.get("X64").unwrap().arch, "x86_64");
        assert_eq!(rows.get("A64").unwrap().cross, "aarch64-linux-gnu-");
    }
}
