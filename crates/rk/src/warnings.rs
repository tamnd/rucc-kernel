//! `rk warnings-diff`: the warnings of every unit in two builds, compared.
//!
//! Under `CONFIG_WERROR` kbuild passes `-Werror`, so a warning the other compiler gives on a unit
//! where the reference gives none stops the build there and nowhere else. A build without it
//! still shows those warnings, so the comparison can be made on either. The warnings come from the
//! standard error each record of `compile.jsonl` keeps, which is the first KiB of it.
//!
//! Units are matched by the object their command writes. Each warning is reduced to its kind: the
//! `-W` option at the end of the line, with `-Werror=` read as the warning it promoted, or the
//! message with names and numbers taken out when no option is named. An assembler `Warning:` is a
//! kind of its own in the same way. A kind the other build gives on a unit where the reference
//! does not is what fails the comparison. A kind only the reference gives is listed and does not
//! fail it, since a compiler that warns less never breaks a build that the reference builds.

use crate::build::{is_unit, shape};
use rk_shim::record::CompileRecord;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// The warning kinds of every unit in a build, by the object it writes. A unit without warnings
/// is here with no kinds, so that the units both builds compiled can be told apart.
pub type Warnings = BTreeMap<String, BTreeSet<String>>;

/// The kind of the warning on one line of standard error, if the line is a warning.
#[must_use]
pub fn kind(line: &str) -> Option<String> {
    let option = line
        .trim_end()
        .strip_suffix(']')
        .and_then(|l| l.rsplit_once('['))
        .map(|(_, o)| o)
        .filter(|o| o.starts_with("-W"))
        .map(|o| {
            let o = o
                .strip_prefix("-Werror=")
                .map_or(o.to_string(), |w| format!("-W{w}"));
            o.trim_end_matches('=').to_string()
        });
    if line.contains(": warning: ") {
        let message = line.split_once(": warning: ").map_or(line, |(_, m)| m);
        return Some(option.unwrap_or_else(|| format!("warning: {}", shape(message))));
    }
    if line.contains(": error: ") {
        // An error is a warning only when `-Werror` made it one, which the option says.
        return option.filter(|_| line.contains("[-Werror"));
    }
    let (_, message) = line.split_once("Warning: ")?;
    Some(format!("as: {}", shape(message)))
}

/// The warning kinds in a compiler's standard error.
#[must_use]
pub fn kinds(stderr: &str) -> BTreeSet<String> {
    stderr.lines().filter_map(kind).collect()
}

/// The object a command writes, from its `-o`.
fn object(record: &CompileRecord) -> Option<&str> {
    let at = record.argv.iter().position(|a| a == "-o")?;
    record.argv.get(at + 1).map(String::as_str)
}

/// The warnings of every unit in the records of a build.
#[must_use]
pub fn collect(records: &[CompileRecord]) -> Warnings {
    let mut warnings = Warnings::new();
    for record in records.iter().filter(|r| is_unit(r)) {
        if let Some(object) = object(record) {
            warnings
                .entry(object.to_string())
                .or_default()
                .extend(kinds(&record.stderr));
        }
    }
    warnings
}

/// What the two builds warned about on the units both compiled.
#[derive(Debug, Default)]
pub struct Comparison {
    /// Units both builds compiled.
    pub units: usize,
    /// Units with a warning in each build.
    pub warned: (usize, usize),
    /// Kinds the other build gives on a unit where the reference does not, with those units.
    pub only_other: BTreeMap<String, Vec<String>>,
    /// Kinds the reference gives on a unit where the other build does not, with those units.
    pub only_reference: BTreeMap<String, Vec<String>>,
    /// Kinds both give on the same unit, with how many units.
    pub shared: BTreeMap<String, usize>,
}

impl Comparison {
    /// Whether the other build gives no warning the reference does not.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.only_other.is_empty()
    }
}

/// Compare the warnings of two builds, unit by unit.
#[must_use]
pub fn compare(reference: &Warnings, other: &Warnings) -> Comparison {
    let mut c = Comparison::default();
    for (object, theirs) in reference {
        let Some(ours) = other.get(object) else {
            continue;
        };
        c.units += 1;
        c.warned.0 += usize::from(!theirs.is_empty());
        c.warned.1 += usize::from(!ours.is_empty());
        for kind in ours.difference(theirs) {
            c.only_other
                .entry(kind.clone())
                .or_default()
                .push(object.clone());
        }
        for kind in theirs.difference(ours) {
            c.only_reference
                .entry(kind.clone())
                .or_default()
                .push(object.clone());
        }
        for kind in theirs.intersection(ours) {
            *c.shared.entry(kind.clone()).or_default() += 1;
        }
    }
    c
}

/// The comparison as Markdown.
#[must_use]
pub fn report(c: &Comparison) -> String {
    let mut s = String::from("### Warnings\n\n");
    let _ = writeln!(
        s,
        "{} units compiled by both. The reference warned on {} of them and the other build on {}.\n",
        c.units, c.warned.0, c.warned.1
    );
    if c.clean() {
        s.push_str("The other build gives no warning the reference does not.\n\n");
    }
    for (label, kinds) in [
        ("Only the other build", &c.only_other),
        ("Only the reference", &c.only_reference),
    ] {
        if kinds.is_empty() {
            continue;
        }
        let _ = writeln!(s, "{label}:\n\n| warning | units | first |\n|---|---|---|");
        let mut rows: Vec<_> = kinds.iter().collect();
        rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
        for (kind, units) in rows.iter().take(60) {
            let _ = writeln!(s, "| `{kind}` | {} | `{}` |", units.len(), units[0]);
        }
        if rows.len() > 60 {
            let _ = writeln!(s, "\nand {} more.", rows.len() - 60);
        }
        s.push('\n');
    }
    if !c.shared.is_empty() {
        let _ = writeln!(s, "Both on the same unit: {}.", c.shared.len());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_warning_is_its_option_and_a_promoted_one_is_the_warning() {
        let gcc = "a.c:1:20: warning: unused variable 'y' [-Wunused-variable]";
        assert_eq!(kind(gcc).as_deref(), Some("-Wunused-variable"));
        let promoted =
            "a.c:2:53: error: array subscript 3 is above array bounds [-Werror=array-bounds=]";
        assert_eq!(kind(promoted).as_deref(), Some("-Warray-bounds"));
        assert_eq!(kind("a.c:1:1: error: unknown type name 'x'"), None);
        assert_eq!(kind("a.c:1:1: note: declared here"), None);
        assert_eq!(kind("cc1: all warnings being treated as errors"), None);
        let bare = "a.c:3:1: warning: 'x' redefined at 12";
        assert_eq!(kind(bare).as_deref(), Some("warning: '_' redefined at N"));
        let gas = "/tmp/ccX.s:40: Warning: 'mov' has 3 operands";
        assert_eq!(kind(gas).as_deref(), Some("as: '_' has N operands"));
    }

    fn unit(object: &str, stderr: &str) -> CompileRecord {
        let line = serde_json::json!({
            "started": 1,
            "argv": ["cc", "-c", "-o", object, format!("/s/{object}.c")],
            "compiler": "/c",
            "cwd": "/o",
            "inputs": [{"path": format!("/s/{object}.c"), "sha256": "a"}],
            "wall-seconds": 0.1,
            "exit": 0,
            "stderr": stderr,
        });
        serde_json::from_value(line).expect("a record")
    }

    #[test]
    fn only_a_warning_the_reference_does_not_give_fails() {
        let reference = collect(&[
            unit("a.o", "a.c:1:1: warning: x [-Wunused-variable]\n"),
            unit("b.o", ""),
            unit("c.o", ""),
        ]);
        let quieter = collect(&[unit("a.o", ""), unit("b.o", "")]);
        let c = compare(&reference, &quieter);
        assert!(c.clean());
        assert_eq!(c.units, 2);
        assert_eq!(
            c.only_reference["-Wunused-variable"],
            vec!["a.o".to_string()]
        );
        let louder = collect(&[
            unit("a.o", "a.c:1:1: warning: x [-Wunused-variable]\n"),
            unit("b.o", "b.c:4:2: warning: y [-Wshadow]\n"),
        ]);
        let c = compare(&reference, &louder);
        assert!(!c.clean());
        assert_eq!(c.only_other["-Wshadow"], vec!["b.o".to_string()]);
        assert_eq!(c.shared["-Wunused-variable"], 1);
        assert!(report(&c).contains("| `-Wshadow` | 1 | `b.o` |"));
    }
}
