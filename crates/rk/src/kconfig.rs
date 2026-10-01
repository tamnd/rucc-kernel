//! `rk config-diff`: the `.config` of two builds of the same version and configuration, compared.
//!
//! Kconfig asks the compiler what it can do, through `cc-option` and the `CC_HAS_` symbols, and a
//! compiler that answers differently from the reference gets a different kernel. K1's exit is that
//! rucc's `.config` is the reference's, apart from the differences listed with a reason in
//! `config-divergences.toml`. This module finds the differences and sorts them into explained and
//! not.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// A `.config`, as symbol to value. `# CONFIG_X is not set` is the value `n`.
pub type Config = BTreeMap<String, String>;

/// Read a `.config`.
#[must_use]
pub fn parse(text: &str) -> Config {
    let mut config = Config::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# CONFIG_") {
            if let Some(symbol) = rest.strip_suffix(" is not set") {
                config.insert(symbol.to_string(), "n".to_string());
            }
        } else if let Some((symbol, value)) = line
            .strip_prefix("CONFIG_")
            .and_then(|rest| rest.split_once('='))
        {
            config.insert(symbol.to_string(), value.to_string());
        }
    }
    config
}

/// Read a `.config` from a file, or from a build directory holding one.
pub fn load(path: &Path) -> Result<Config, String> {
    let file = if path.is_dir() {
        path.join(".config")
    } else {
        path.to_path_buf()
    };
    let text =
        std::fs::read_to_string(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
    Ok(parse(&text))
}

/// One symbol that differs. A side with no value did not have the symbol at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// The symbol, without `CONFIG_`.
    pub symbol: String,
    /// The reference's value.
    pub reference: Option<String>,
    /// The other build's value.
    pub other: Option<String>,
    /// Why, when `config-divergences.toml` says.
    pub reason: Option<String>,
}

/// Every symbol whose value differs, in symbol order.
#[must_use]
pub fn diff(reference: &Config, other: &Config, divergences: &Divergences) -> Vec<Difference> {
    let symbols: BTreeSet<&String> = reference.keys().chain(other.keys()).collect();
    symbols
        .into_iter()
        .filter(|s| reference.get(*s) != other.get(*s))
        .map(|s| Difference {
            symbol: s.clone(),
            reference: reference.get(s).cloned(),
            other: other.get(s).cloned(),
            reason: divergences.reason(s),
        })
        .collect()
}

/// `config-divergences.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Divergences {
    /// Every explained difference.
    #[serde(default, rename = "divergence")]
    pub divergences: Vec<Divergence>,
    /// Every explained difference in the flags of units, for `rk flags-diff`.
    #[serde(default)]
    pub flags: Vec<FlagRule>,
}

/// A flag that may differ between the two builds' command lines, and why.
#[derive(Debug, Clone, Deserialize)]
pub struct FlagRule {
    /// The flag as `rk flags-diff` prints it. A trailing `*` matches any flag with that prefix.
    pub flag: String,
    /// Why it differs, with the rucc issue that removes it if there is one.
    pub reason: String,
}

/// A difference that is expected, and why.
#[derive(Debug, Clone, Deserialize)]
pub struct Divergence {
    /// The symbol, without `CONFIG_`. A trailing `*` matches any symbol with that prefix.
    pub symbol: String,
    /// Why it differs, with the rucc issue that removes it if there is one.
    pub reason: String,
}

impl Divergences {
    /// Read `config-divergences.toml`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Read the text of `config-divergences.toml`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let divergences: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        for d in &divergences.divergences {
            if d.reason.trim().is_empty() {
                return Err(format!("{} has no reason", d.symbol));
            }
        }
        for f in &divergences.flags {
            if f.reason.trim().is_empty() {
                return Err(format!("{} has no reason", f.flag));
            }
        }
        Ok(divergences)
    }

    /// The reason a symbol may differ, if there is one.
    #[must_use]
    pub fn reason(&self, symbol: &str) -> Option<String> {
        self.divergences
            .iter()
            .find(|d| match d.symbol.strip_suffix('*') {
                Some(prefix) => symbol.starts_with(prefix),
                None => d.symbol == symbol,
            })
            .map(|d| d.reason.clone())
    }

    /// The reason a flag may differ, if there is one.
    #[must_use]
    pub fn flag_reason(&self, flag: &str) -> Option<String> {
        self.flags
            .iter()
            .find(|f| match f.flag.strip_suffix('*') {
                Some(prefix) => flag.starts_with(prefix),
                None => f.flag == flag,
            })
            .map(|f| f.reason.clone())
    }
}

/// A configuration fragment: symbols and the values it asks for, in order. Values are cut at a
/// `#` that follows whitespace, so that a line may carry a comment, which a `.config` may not.
#[must_use]
pub fn parse_fragment(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if let Some(symbol) = line
                .strip_prefix("# CONFIG_")
                .and_then(|r| r.strip_suffix(" is not set"))
            {
                return Some((symbol.to_string(), "n".to_string()));
            }
            let (symbol, value) = line.strip_prefix("CONFIG_")?.split_once('=')?;
            let value = value
                .find(" #")
                .or_else(|| value.find("\t#"))
                .map_or(value, |at| &value[..at]);
            Some((symbol.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// A `.config` with a fragment laid over it, as `merge_config.sh -m` does: every line that sets
/// a symbol the fragment names is dropped, and the fragment's lines go at the end. Running
/// `olddefconfig` afterwards settles dependencies.
#[must_use]
pub fn merge(config: &str, fragment: &[(String, String)]) -> String {
    let named = |line: &str| {
        let line = line.trim();
        let symbol = line
            .strip_prefix("# CONFIG_")
            .and_then(|r| r.strip_suffix(" is not set"))
            .or_else(|| {
                line.strip_prefix("CONFIG_")
                    .and_then(|r| r.split_once('='))
                    .map(|(s, _)| s)
            });
        symbol.is_some_and(|s| fragment.iter().any(|(f, _)| f == s))
    };
    let mut out: String = config
        .lines()
        .filter(|l| !named(l))
        .flat_map(|l| [l, "\n"])
        .collect();
    for (symbol, value) in fragment {
        if value == "n" {
            let _ = writeln!(out, "# CONFIG_{symbol} is not set");
        } else {
            let _ = writeln!(out, "CONFIG_{symbol}={value}");
        }
    }
    out
}

/// The fragment's requests that did not take effect, usually because the symbol does not exist
/// in this version or on this architecture, or depends on something that is off. A symbol asked
/// to be `n` that is missing counts as taken.
#[must_use]
pub fn missed(config: &Config, fragment: &[(String, String)]) -> Vec<String> {
    fragment
        .iter()
        .filter(|(symbol, value)| match config.get(symbol) {
            Some(v) => v != value,
            None => value != "n",
        })
        .map(|(symbol, _)| symbol.clone())
        .collect()
}

/// The options a starting `.config` turned on, as `y` or `m`, that are off after `olddefconfig`.
/// For a distribution config these are what this toolchain could not give the kernel the
/// distribution built, such as BTF without pahole or Rust without rustc.
#[must_use]
pub fn dropped(before: &Config, after: &Config) -> Vec<String> {
    let on = |v: Option<&String>| v.is_some_and(|v| v == "y" || v == "m");
    before
        .iter()
        .filter(|(symbol, value)| on(Some(value)) && !on(after.get(*symbol)))
        .map(|(symbol, _)| symbol.clone())
        .collect()
}

/// A value for a table cell: missing symbols show as a dash.
fn cell(value: Option<&String>) -> String {
    value.map_or_else(|| "-".to_string(), |v| format!("`{v}`"))
}

/// The differences as a markdown report, unexplained first.
#[must_use]
pub fn report(differences: &[Difference]) -> String {
    let (explained, unexplained): (Vec<&Difference>, Vec<&Difference>) =
        differences.iter().partition(|d| d.reason.is_some());
    let mut s = format!(
        "{} symbols differ, {} explained and {} not.\n",
        differences.len(),
        explained.len(),
        unexplained.len()
    );
    if !unexplained.is_empty() {
        s.push_str("\n| symbol | reference | other |\n|---|---|---|\n");
        for d in &unexplained {
            let _ = writeln!(
                s,
                "| {} | {} | {} |",
                d.symbol,
                cell(d.reference.as_ref()),
                cell(d.other.as_ref())
            );
        }
    }
    if !explained.is_empty() {
        s.push_str("\nExplained:\n\n| symbol | reference | other | reason |\n|---|---|---|---|\n");
        for d in &explained {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                d.symbol,
                cell(d.reference.as_ref()),
                cell(d.other.as_ref()),
                d.reason.as_deref().unwrap_or_default()
            );
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const GCC: &str = "\
CONFIG_CC_VERSION_TEXT=\"gcc-14 (Ubuntu 14.2.0-4ubuntu2~24.04) 14.2.0\"
CONFIG_CC_IS_GCC=y
CONFIG_GCC_VERSION=140200
CONFIG_CC_HAS_ASM_GOTO_OUTPUT=y
# CONFIG_KASAN is not set
CONFIG_STACKPROTECTOR=y
";

    const RUCC: &str = "\
CONFIG_CC_VERSION_TEXT=\"gcc (rucc 0.17.0, GNU C persona 14.2.0) 14.2.0\"
CONFIG_CC_IS_GCC=y
CONFIG_GCC_VERSION=140200
# CONFIG_KASAN is not set
# CONFIG_STACKPROTECTOR is not set
";

    #[test]
    fn set_unset_and_missing_symbols_are_read() {
        let config = parse(GCC);
        assert_eq!(config["KASAN"], "n");
        assert_eq!(config["GCC_VERSION"], "140200");
        assert_eq!(config.len(), 6);
    }

    #[test]
    fn differences_are_sorted_into_explained_and_not() {
        let divergences = Divergences::parse(
            "[[divergence]]\nsymbol = \"CC_VERSION_TEXT\"\nreason = \"the banner names rucc\"\n",
        )
        .unwrap();
        let found = diff(&parse(GCC), &parse(RUCC), &divergences);
        let names: Vec<&str> = found.iter().map(|d| d.symbol.as_str()).collect();
        assert_eq!(
            names,
            [
                "CC_HAS_ASM_GOTO_OUTPUT",
                "CC_VERSION_TEXT",
                "STACKPROTECTOR"
            ]
        );
        assert_eq!(found[0].other, None);
        assert!(found[1].reason.is_some());
        assert!(found[2].reason.is_none());
        assert!(report(&found).starts_with("3 symbols differ, 1 explained and 2 not."));
    }

    #[test]
    fn a_star_matches_a_prefix() {
        let divergences =
            Divergences::parse("[[divergence]]\nsymbol = \"CC_HAS_*\"\nreason = \"probes\"\n")
                .unwrap();
        assert!(divergences.reason("CC_HAS_ASM_GOTO_OUTPUT").is_some());
        assert!(divergences.reason("CC_IS_GCC").is_none());
    }

    #[test]
    fn a_divergence_needs_a_reason() {
        assert!(Divergences::parse("[[divergence]]\nsymbol = \"X\"\nreason = \" \"\n").is_err());
    }

    #[test]
    fn a_fragment_replaces_what_it_names_and_reports_what_did_not_stick() {
        let fragment = parse_fragment(
            "# console\nCONFIG_STACKPROTECTOR=y   # on\nCONFIG_SERIAL_AMBA_PL011=y\n# CONFIG_KASAN is not set\n",
        );
        assert_eq!(fragment.len(), 3);
        assert_eq!(fragment[0], ("STACKPROTECTOR".to_string(), "y".to_string()));
        let merged = merge(RUCC, &fragment);
        assert!(!merged.contains("# CONFIG_STACKPROTECTOR is not set"));
        assert!(merged.ends_with("CONFIG_SERIAL_AMBA_PL011=y\n# CONFIG_KASAN is not set\n"));
        let settled = parse(RUCC);
        assert_eq!(
            missed(&settled, &fragment),
            ["STACKPROTECTOR", "SERIAL_AMBA_PL011"]
        );
    }

    #[test]
    fn the_committed_fragments_read() {
        for era in ["E8", "E9", "E10", "E11"] {
            let path = format!(
                "{}/../../configs/test.fragment.{era}",
                env!("CARGO_MANIFEST_DIR")
            );
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(parse_fragment(&text).len() > 20, "{path}");
        }
    }

    #[test]
    fn the_committed_divergences_read() {
        Divergences::parse(include_str!("../../../config-divergences.toml")).unwrap();
    }

    #[test]
    fn dropped_lists_what_olddefconfig_turned_off() {
        let before = parse(
            "CONFIG_RUST=y\nCONFIG_BTF=y\nCONFIG_E1000=m\nCONFIG_SALT=\"x\"\n# CONFIG_KASAN is not set\n",
        );
        let after =
            parse("CONFIG_BTF=y\n# CONFIG_E1000 is not set\nCONFIG_SALT=\"\"\nCONFIG_KASAN=y\n");
        assert_eq!(dropped(&before, &after), ["E1000", "RUST"]);
    }
}
