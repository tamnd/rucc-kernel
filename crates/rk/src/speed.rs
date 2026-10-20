//! `rk speed`: how fast and how large two compilers are on the same kernel, from the times and the
//! peak resident sets the shim writes into each build's `compile.jsonl`.
//!
//! A unit is the object a call writes, so a unit is counted only when both builds compiled it and
//! both calls succeeded. Probes are left out, because they write nothing the kernel keeps, and so
//! are calls a rucc build delegated to another compiler. The time of a unit is the user and system
//! CPU time of the compiler and everything it ran, which a busy machine changes much less than the
//! wall clock. The sum of the wall clock is given too, for a machine that does nothing else.

use rk_shim::record::CompileRecord;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// What one call cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cost {
    /// User and system CPU seconds.
    pub cpu: f64,
    /// Wall clock seconds.
    pub wall: f64,
    /// Peak resident set in KiB.
    pub rss_kb: u64,
}

/// The cost of every unit of one build, by the object it writes.
pub type Units = BTreeMap<String, Cost>;

/// The units of a list of records. A unit compiled more than once keeps its last cost.
#[must_use]
pub fn units(records: &[CompileRecord]) -> Units {
    let mut units = Units::new();
    for r in records {
        if r.probe || r.delegated.is_some() || !r.succeeded() {
            continue;
        }
        let Some(object) =
            r.outputs.iter().map(|o| o.path.as_str()).find(|p| {
                Path::new(p).extension().is_some_and(|e| e == "o") && !p.starts_with('/')
            })
        else {
            continue;
        };
        let cpu = r.user_seconds.unwrap_or(0.0) + r.system_seconds.unwrap_or(0.0);
        units.insert(
            object.to_string(),
            Cost {
                cpu,
                wall: r.wall_seconds,
                rss_kb: r.peak_rss_kb.unwrap_or(0),
            },
        );
    }
    units
}

/// The units of a build directory.
pub fn load(dir: &Path) -> Result<Units, String> {
    let path = dir.join("compile.jsonl");
    let (records, _) =
        rk_shim::record::read_log(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    Ok(units(&records))
}

/// One side's totals over the shared units.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Totals {
    pub cpu: f64,
    pub wall: f64,
    /// The largest peak resident set of any unit.
    pub rss_max_kb: u64,
    /// The median peak resident set.
    pub rss_median_kb: u64,
}

/// A unit and what it cost on each side.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub object: String,
    pub reference: Cost,
    pub other: Cost,
}

/// The comparison of two builds.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// Units both builds compiled.
    pub shared: usize,
    /// Units only one side compiled.
    pub only_reference: usize,
    pub only_other: usize,
    pub reference: Totals,
    pub other: Totals,
    /// The shared units, the most CPU time on the other side first.
    pub units: Vec<Unit>,
}

impl Comparison {
    /// How many times faster the other side is in CPU time. Above 1 when it is faster.
    #[must_use]
    pub fn speedup(&self) -> f64 {
        if self.other.cpu > 0.0 {
            self.reference.cpu / self.other.cpu
        } else {
            0.0
        }
    }
}

fn totals<'a>(costs: impl Iterator<Item = &'a Cost>) -> Totals {
    let mut t = Totals::default();
    let mut rss = Vec::new();
    for c in costs {
        t.cpu += c.cpu;
        t.wall += c.wall;
        rss.push(c.rss_kb);
    }
    rss.sort_unstable();
    t.rss_max_kb = rss.last().copied().unwrap_or(0);
    t.rss_median_kb = rss.get(rss.len() / 2).copied().unwrap_or(0);
    t
}

/// Compare two builds over the units both compiled.
#[must_use]
pub fn compare(reference: &Units, other: &Units) -> Comparison {
    let mut units: Vec<Unit> = reference
        .iter()
        .filter_map(|(object, r)| {
            other.get(object).map(|o| Unit {
                object: object.clone(),
                reference: *r,
                other: *o,
            })
        })
        .collect();
    units.sort_by(|a, b| b.other.cpu.total_cmp(&a.other.cpu));
    Comparison {
        shared: units.len(),
        only_reference: reference.keys().filter(|k| !other.contains_key(*k)).count(),
        only_other: other.keys().filter(|k| !reference.contains_key(*k)).count(),
        reference: totals(units.iter().map(|u| &u.reference)),
        other: totals(units.iter().map(|u| &u.other)),
        units,
    }
}

fn mib(kb: u64) -> f64 {
    f64::from(u32::try_from(kb).unwrap_or(u32::MAX)) / 1024.0
}

/// The comparison as Markdown, with the `top` units the other side spends most time on and the
/// `top` units where it is slowest against the reference.
#[must_use]
pub fn report(c: &Comparison, reference: &str, other: &str, top: usize) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{} units compiled by both, {} only by {reference}, {} only by {other}.\n",
        c.shared, c.only_reference, c.only_other
    );
    let _ = writeln!(s, "| | {reference} | {other} |\n|---|---|---|");
    let _ = writeln!(
        s,
        "| CPU seconds | {:.1} | {:.1} |",
        c.reference.cpu, c.other.cpu
    );
    let _ = writeln!(
        s,
        "| wall seconds, summed | {:.1} | {:.1} |",
        c.reference.wall, c.other.wall
    );
    let _ = writeln!(
        s,
        "| peak memory, largest unit | {:.0} MiB | {:.0} MiB |",
        mib(c.reference.rss_max_kb),
        mib(c.other.rss_max_kb)
    );
    let _ = writeln!(
        s,
        "| peak memory, median unit | {:.0} MiB | {:.0} MiB |",
        mib(c.reference.rss_median_kb),
        mib(c.other.rss_median_kb)
    );
    let _ = writeln!(
        s,
        "\n{other} uses {:.2}x less CPU time than {reference}.",
        c.speedup()
    );
    if top == 0 || c.units.is_empty() {
        return s;
    }
    let row = |s: &mut String, u: &Unit| {
        let ratio = if u.reference.cpu > 0.0 {
            u.other.cpu / u.reference.cpu
        } else {
            0.0
        };
        let _ = writeln!(
            s,
            "| `{}` | {:.2} | {:.2} | {ratio:.2} |",
            u.object, u.reference.cpu, u.other.cpu
        );
    };
    let _ = writeln!(
        s,
        "\nThe units {other} spends most CPU time on:\n\n| unit | {reference} s | {other} s | ratio |\n|---|---|---|---|"
    );
    for u in c.units.iter().take(top) {
        row(&mut s, u);
    }
    let mut slow: Vec<&Unit> = c.units.iter().filter(|u| u.reference.cpu >= 0.05).collect();
    slow.sort_by(|a, b| {
        (b.other.cpu / b.reference.cpu).total_cmp(&(a.other.cpu / a.reference.cpu))
    });
    let _ = writeln!(
        s,
        "\nThe units where {other} is slowest against {reference}, of those that take {reference} 0.05 s or more:\n\n| unit | {reference} s | {other} s | ratio |\n|---|---|---|---|"
    );
    for u in slow.into_iter().take(top) {
        row(&mut s, u);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = r#"{"started":1,"argv":["rk-cc","-c","-x","c","/dev/null","-o",".tmp_1/tmp"],"compiler":"/g","cwd":"/o","wall-seconds":0.5,"user-seconds":0.4,"exit":0,"probe":true}
{"started":1,"argv":["rk-cc","-c","-o","kernel/fork.o","/s/kernel/fork.c"],"compiler":"/g","cwd":"/o","outputs":[{"path":"kernel/fork.o","sha256":"a"}],"wall-seconds":2.0,"user-seconds":1.5,"system-seconds":0.5,"peak-rss-kb":102400,"exit":0}
{"started":1,"argv":["rk-cc","-c","-o","mm/slub.o","/s/mm/slub.c"],"compiler":"/g","cwd":"/o","outputs":[{"path":"mm/slub.o","sha256":"a"}],"wall-seconds":1.0,"user-seconds":0.8,"system-seconds":0.2,"peak-rss-kb":51200,"exit":0}
{"started":1,"argv":["rk-cc","-c","-o","lib/only.o","/s/lib/only.c"],"compiler":"/g","cwd":"/o","outputs":[{"path":"lib/only.o","sha256":"a"}],"wall-seconds":1.0,"user-seconds":1.0,"exit":0}
"#;

    const OTHER: &str = r#"{"started":1,"argv":["rk-cc","-c","-o","kernel/fork.o","/s/kernel/fork.c"],"compiler":"/r","cwd":"/o","outputs":[{"path":"kernel/fork.o","sha256":"b"}],"wall-seconds":0.6,"user-seconds":0.4,"system-seconds":0.1,"peak-rss-kb":20480,"exit":0}
{"started":1,"argv":["rk-cc","-c","-o","mm/slub.o","/s/mm/slub.c"],"compiler":"/r","cwd":"/o","outputs":[{"path":"mm/slub.o","sha256":"b"}],"wall-seconds":0.6,"user-seconds":0.4,"system-seconds":0.1,"peak-rss-kb":10240,"exit":0}
{"started":1,"argv":["rk-cc","-m16","-c","-o","arch/x86/boot/a20.o","/s/a20.c"],"compiler":"/g","cwd":"/o","outputs":[{"path":"arch/x86/boot/a20.o","sha256":"b"}],"wall-seconds":9.0,"user-seconds":9.0,"exit":0,"delegated":"m16"}
{"started":1,"argv":["rk-cc","-c","-o","lib/bad.o","/s/lib/bad.c"],"compiler":"/r","cwd":"/o","wall-seconds":0.1,"exit":1}
"#;

    fn parse(text: &str) -> Units {
        units(&rk_shim::record::parse_log(text).0)
    }

    #[test]
    fn probes_failures_and_delegated_calls_are_not_units() {
        let r = parse(REFERENCE);
        assert_eq!(
            r.keys().collect::<Vec<_>>(),
            ["kernel/fork.o", "lib/only.o", "mm/slub.o"]
        );
        let o = parse(OTHER);
        assert_eq!(o.keys().collect::<Vec<_>>(), ["kernel/fork.o", "mm/slub.o"]);
    }

    #[test]
    fn only_the_units_both_compiled_are_compared() {
        let c = compare(&parse(REFERENCE), &parse(OTHER));
        assert_eq!((c.shared, c.only_reference, c.only_other), (2, 1, 0));
        assert!((c.reference.cpu - 3.0).abs() < 1e-9);
        assert!((c.other.cpu - 1.0).abs() < 1e-9);
        assert!((c.speedup() - 3.0).abs() < 1e-9);
        assert_eq!(c.reference.rss_max_kb, 102_400);
        assert_eq!(c.other.rss_median_kb, 20_480);
        assert_eq!(c.units[0].object, "kernel/fork.o");
    }

    #[test]
    fn the_report_names_the_speedup() {
        let c = compare(&parse(REFERENCE), &parse(OTHER));
        let text = report(&c, "gcc-16", "rucc", 5);
        assert!(text.contains("rucc uses 3.00x less CPU time than gcc-16."));
        assert!(text.contains("| CPU seconds | 3.0 | 1.0 |"));
        assert!(text.contains("| `mm/slub.o` | 1.00 | 0.50 | 0.50 |"));
    }
}
