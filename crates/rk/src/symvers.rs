//! `rk symvers-diff`: `Module.symvers` and the global symbols of `System.map` in two builds,
//! compared.
//!
//! `Module.symvers` holds every exported symbol with its CRC, the module that exports it, the
//! kind of export and, from 5.4, its namespace. The CRC is genksyms' hash of the symbol's type as
//! the preprocessor left it, so a difference there means the two preprocessors left a
//! different type behind, and a module built against one kernel will not load into the
//! other. `System.map` is compared by its global symbols only, since which static functions
//! survive depends on inlining and the local names carry the compiler's clone suffixes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// One export: CRC, module, kind and namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    /// The CRC as written.
    pub crc: String,
    /// The module, `vmlinux` for the kernel itself.
    pub module: String,
    /// `EXPORT_SYMBOL`, `EXPORT_SYMBOL_GPL` and so on.
    pub kind: String,
    /// The namespace, empty when there is none.
    pub namespace: String,
}

/// Parse `Module.symvers`, by symbol. The namespace column is new in 5.4, and between 5.4 and
/// 5.9 it came before the module rather than last.
#[must_use]
pub fn parse_symvers(text: &str) -> BTreeMap<String, Export> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (crc, symbol) = match f.as_slice() {
            [crc, symbol, ..] => (*crc, *symbol),
            _ => continue,
        };
        let (module, kind, namespace) = match f.as_slice() {
            [_, _, module, kind] => (*module, *kind, ""),
            [_, _, a, b, c] if b.starts_with("EXPORT_") => (*a, *b, *c),
            [_, _, namespace, module, kind] => (*module, *kind, *namespace),
            _ => ("", "", ""),
        };
        out.insert(
            symbol.to_string(),
            Export {
                crc: crc.to_string(),
                module: module.to_string(),
                kind: kind.to_string(),
                namespace: namespace.to_string(),
            },
        );
    }
    out
}

/// The global symbols of `System.map`, the lines whose type letter is upper case.
#[must_use]
pub fn parse_map(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let (_, kind, name) = (f.next()?, f.next()?, f.next()?);
            kind.chars()
                .all(|c| c.is_ascii_uppercase())
                .then(|| name.to_string())
        })
        .collect()
}

/// What differs.
#[derive(Debug, Clone, Default)]
pub struct Comparison {
    /// Exports only the reference has.
    pub exports_missing: Vec<String>,
    /// Exports only the other build has.
    pub exports_extra: Vec<String>,
    /// Exports whose CRC, module, kind or namespace differ, with both sides.
    pub exports_changed: Vec<(String, Export, Export)>,
    /// Global symbols only the reference has.
    pub globals_missing: Vec<String>,
    /// Global symbols only the other build has.
    pub globals_extra: Vec<String>,
    /// The number of exports and globals in the reference.
    pub counted: (usize, usize),
}

impl Comparison {
    /// Whether nothing differs.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.exports_missing.is_empty()
            && self.exports_extra.is_empty()
            && self.exports_changed.is_empty()
            && self.globals_missing.is_empty()
            && self.globals_extra.is_empty()
    }
}

/// Compare two builds' exports and global symbols.
#[must_use]
pub fn compare(
    reference: (&BTreeMap<String, Export>, &BTreeSet<String>),
    other: (&BTreeMap<String, Export>, &BTreeSet<String>),
) -> Comparison {
    let (rs, rm) = reference;
    let (os, om) = other;
    Comparison {
        exports_missing: rs
            .keys()
            .filter(|k| !os.contains_key(*k))
            .cloned()
            .collect(),
        exports_extra: os
            .keys()
            .filter(|k| !rs.contains_key(*k))
            .cloned()
            .collect(),
        exports_changed: rs
            .iter()
            .filter_map(|(k, a)| {
                let b = os.get(k)?;
                (a != b).then(|| (k.clone(), a.clone(), b.clone()))
            })
            .collect(),
        globals_missing: rm.difference(om).cloned().collect(),
        globals_extra: om.difference(rm).cloned().collect(),
        counted: (rs.len(), rm.len()),
    }
}

/// Read `Module.symvers` and `System.map` from a build directory. A missing file reads as
/// empty, and the caller decides whether that is an error.
pub fn load(dir: &Path) -> (BTreeMap<String, Export>, BTreeSet<String>) {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
    (
        parse_symvers(&read("Module.symvers")),
        parse_map(&read("System.map")),
    )
}

fn names(list: &[String]) -> String {
    const SHOWN: usize = 40;
    let mut s = list
        .iter()
        .take(SHOWN)
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if list.len() > SHOWN {
        let _ = write!(s, " and {} more", list.len() - SHOWN);
    }
    s
}

/// The comparison as markdown.
#[must_use]
pub fn report(c: &Comparison) -> String {
    let mut s = String::from("### Module.symvers and System.map\n\n");
    let _ = writeln!(
        s,
        "{} exports and {} global symbols in the reference.\n",
        c.counted.0, c.counted.1
    );
    if c.clean() {
        s.push_str("Both builds export the same symbols with the same CRCs.\n");
        return s;
    }
    for (label, list) in [
        ("Exports missing", &c.exports_missing),
        ("Exports only in the other build", &c.exports_extra),
        ("Global symbols missing", &c.globals_missing),
        ("Global symbols only in the other build", &c.globals_extra),
    ] {
        if !list.is_empty() {
            let _ = writeln!(s, "{label} ({}): {}\n", list.len(), names(list));
        }
    }
    if !c.exports_changed.is_empty() {
        s.push_str("| symbol | reference | other |\n|---|---|---|\n");
        let show = |e: &Export| format!("{} {} {} {}", e.crc, e.module, e.kind, e.namespace);
        for (name, a, b) in c.exports_changed.iter().take(60) {
            let _ = writeln!(s, "| `{name}` | {} | {} |", show(a).trim(), show(b).trim());
        }
        if c.exports_changed.len() > 60 {
            let _ = writeln!(s, "\nand {} more.", c.exports_changed.len() - 60);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symvers_layout_reads() {
        let old = parse_symvers("0x1234abcd\tkmalloc\tvmlinux\tEXPORT_SYMBOL\n");
        assert_eq!(old["kmalloc"].module, "vmlinux");
        assert_eq!(old["kmalloc"].namespace, "");
        let middle = parse_symvers("0x1\tfoo\tFOO_NS\tdrivers/foo\tEXPORT_SYMBOL_GPL\n");
        assert_eq!(middle["foo"].namespace, "FOO_NS");
        assert_eq!(middle["foo"].module, "drivers/foo");
        let new = parse_symvers("0x1\tfoo\tdrivers/foo\tEXPORT_SYMBOL_GPL\tFOO_NS\n");
        assert_eq!(new["foo"], middle["foo"]);
    }

    #[test]
    fn a_changed_crc_and_a_lost_global_are_found() {
        let rs = parse_symvers("0x1\ta\tvmlinux\tEXPORT_SYMBOL\n0x2\tb\tvmlinux\tEXPORT_SYMBOL\n");
        let os = parse_symvers("0x1\ta\tvmlinux\tEXPORT_SYMBOL\n0x3\tb\tvmlinux\tEXPORT_SYMBOL\n");
        let rm = parse_map("ffff T start_kernel\nffff t helper.isra.0\nffff D jiffies\n");
        let om = parse_map("ffff T start_kernel\nffff t helper\n");
        assert_eq!(rm.len(), 2);
        let c = compare((&rs, &rm), (&os, &om));
        assert_eq!(c.exports_changed.len(), 1);
        assert_eq!(c.globals_missing, ["jiffies"]);
        assert!(!c.clean());
        let text = report(&c);
        assert!(text.contains("| `b` | 0x2 vmlinux EXPORT_SYMBOL | 0x3 vmlinux EXPORT_SYMBOL |"));
        assert!(compare((&rs, &rm), (&rs, &rm)).clean());
    }
}
