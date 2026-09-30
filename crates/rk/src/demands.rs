//! `rk demands`: the demand census, which is what rucc has to do next, ranked by how many units
//! it blocks.
//!
//! Every failed unit in every build given is reduced to the key of its first error, as in
//! `build::error_key`, and the keys are counted. A key that blocks four hundred units in three
//! configurations is worth more than one that blocks two, and the table says which. Each row
//! carries one example unit so that the reduction for the rucc issue has a place to start.

use crate::build::{error_key, is_unit};
use rk_shim::record::CompileRecord;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// One demand: an error key and where it was seen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Demand {
    /// The normalized error.
    pub key: String,
    /// Units that failed with it, counted once per build.
    pub units: usize,
    /// The builds it was seen in.
    pub builds: BTreeSet<String>,
    /// The first unit seen with it, with the unnormalized error line.
    pub example: (String, String),
}

/// Add one build's failed units to the census.
pub fn add(census: &mut BTreeMap<String, Demand>, build: &str, records: &[CompileRecord]) {
    for record in records.iter().filter(|r| is_unit(r) && !r.succeeded()) {
        let key = error_key(&record.stderr).unwrap_or_else(|| {
            record.signal.map_or_else(
                || "(no error line)".to_string(),
                |s| format!("(killed by signal {s})"),
            )
        });
        let demand = census.entry(key.clone()).or_insert_with(|| Demand {
            key,
            ..Demand::default()
        });
        demand.units += 1;
        demand.builds.insert(build.to_string());
        if demand.example.0.is_empty() {
            let unit = record
                .inputs
                .iter()
                .map(|i| i.path.as_str())
                .find(|p| {
                    Path::new(p)
                        .extension()
                        .is_some_and(|e| e == "c" || e == "S")
                })
                .unwrap_or_default();
            let line = record
                .stderr
                .lines()
                .find(|l| l.contains("error"))
                .unwrap_or_default();
            demand.example = (unit.to_string(), line.trim().to_string());
        }
    }
}

/// The census, most units first.
#[must_use]
pub fn ranked(census: BTreeMap<String, Demand>) -> Vec<Demand> {
    let mut out: Vec<Demand> = census.into_values().collect();
    out.sort_by(|a, b| {
        b.units
            .cmp(&a.units)
            .then(b.builds.len().cmp(&a.builds.len()))
            .then(a.key.cmp(&b.key))
    });
    out
}

/// The census as markdown, the first `limit` rows.
#[must_use]
pub fn report(demands: &[Demand], limit: usize) -> String {
    let total: usize = demands.iter().map(|d| d.units).sum();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{total} failed units, {} distinct errors.",
        demands.len()
    );
    if demands.is_empty() {
        return s;
    }
    let _ = writeln!(
        s,
        "\n| units | builds | error | example |\n|---|---|---|---|"
    );
    let escape = |t: &str| t.replace('|', "\\|");
    for d in demands.iter().take(limit) {
        let _ = writeln!(
            s,
            "| {} | {} | `{}` | `{}` |",
            d.units,
            d.builds.len(),
            escape(&d.key),
            escape(&d.example.0)
        );
    }
    if demands.len() > limit {
        let _ = writeln!(s, "\nand {} more.", demands.len() - limit);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use rk_shim::record::parse_log;

    const ONE: &str = r#"{"started":1,"argv":["cc","-c","-o","a.o","/s/a.c"],"compiler":"/r","cwd":"/o","inputs":[{"path":"/s/a.c","sha256":"x"}],"wall-seconds":0.1,"exit":1,"stderr":"/s/a.c:1:1: error: unknown attribute 'cold'\n"}
{"started":1,"argv":["cc","-c","-o","b.o","/s/b.c"],"compiler":"/r","cwd":"/o","inputs":[{"path":"/s/b.c","sha256":"x"}],"wall-seconds":0.1,"exit":1,"stderr":"/s/b.c:9:1: error: unknown attribute 'section'\n"}
{"started":1,"argv":["cc","-c","-o","c.o","/s/c.S"],"compiler":"/r","cwd":"/o","inputs":[{"path":"/s/c.S","sha256":"x"}],"wall-seconds":0.1,"exit":1,"stderr":"/s/c.S:2: error: unknown directive .pushsection\n"}
"#;

    const TWO: &str = r#"{"started":1,"argv":["cc","-c","-o","d.o","/s/d.c"],"compiler":"/r","cwd":"/o","inputs":[{"path":"/s/d.c","sha256":"x"}],"wall-seconds":0.1,"exit":1,"stderr":"/s/d.c:1:1: error: unknown attribute 'noinline'\n"}
"#;

    #[test]
    fn demands_rank_by_units_across_builds() {
        let mut census = BTreeMap::new();
        add(&mut census, "tinyconfig", &parse_log(ONE).0);
        add(&mut census, "defconfig", &parse_log(TWO).0);
        let demands = ranked(census);
        assert_eq!(demands.len(), 2);
        assert_eq!(demands[0].key, "unknown attribute '_'");
        assert_eq!(demands[0].units, 3);
        assert_eq!(demands[0].builds.len(), 2);
        assert_eq!(demands[0].example.0, "/s/a.c");
        assert!(report(&demands, 1).contains("and 1 more."));
    }
}
