//! KUnit's TAP output, read off the serial console.
//!
//! KUnit prints KTAP (TAP version 14 before 5.18) to the kernel log, and with the console at its
//! default log level every line reaches the serial port behind a printk timestamp. A suite looks
//! like this, with each level of nesting indented by four spaces, or by a tab in old kernels:
//!
//! ```text
//! KTAP version 1
//! 1..1
//!     KTAP version 1
//!     # Subtest: example
//!     1..2
//!     ok 1 example_simple_test
//!     ok 2 example_skip_test # SKIP this test should be skipped
//! ok 1 example
//! ```
//!
//! This does what `tools/testing/kunit/kunit_parser.py` does for the part the harness needs: every
//! result becomes a unit named by the path of subtests it sits in, so the case above is
//! `example.example_simple_test` and the suite itself is `example`. Parameterised cases nest one
//! more level. Lines that are not TAP, which on a console are most of them, are skipped, so the
//! parse survives other kernel messages in between.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A unit's result, from best to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// `ok`.
    Pass,
    /// `ok` or `not ok` with a `# SKIP` directive.
    Skip,
    /// `not ok`.
    Fail,
    /// Not in the output at all, because the run stopped before it or never had it.
    Missing,
}

impl Status {
    /// The word for a report.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skip => "skip",
            Self::Missing => "missing",
        }
    }
}

/// How deep a line is indented, a tab counting as eight spaces.
fn indent(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 8 } else { 1 })
        .sum()
}

/// Read a result line, `ok 1 name` or `not ok 2 - name # SKIP why`, as its name and status.
#[must_use]
pub fn parse_result(text: &str) -> Option<(String, Status)> {
    let (ok, rest) = if let Some(rest) = text.strip_prefix("not ok ") {
        (false, rest)
    } else {
        (true, text.strip_prefix("ok ")?)
    };
    let (number, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let rest = rest.strip_prefix("- ").unwrap_or(rest);
    let (name, directive) = match rest.split_once(" #") {
        Some((name, directive)) => (name, directive.trim()),
        None => (rest, ""),
    };
    let status = if directive.to_ascii_uppercase().starts_with("SKIP") {
        Status::Skip
    } else if ok {
        Status::Pass
    } else {
        Status::Fail
    };
    let name = name.trim();
    (!name.is_empty()).then(|| (name.to_string(), status))
}

/// Every KUnit result in a console log, by path. The printk timestamp is taken off each line
/// first. When a path shows up twice, the worse result is kept.
#[must_use]
pub fn parse(console: &str) -> BTreeMap<String, Status> {
    let mut units: BTreeMap<String, Status> = BTreeMap::new();
    let mut open: Vec<(usize, String)> = Vec::new();
    for line in console.lines() {
        let line = crate::boot::strip_timestamp(line.trim_end_matches('\r'));
        let depth = indent(line);
        let text = line.trim_start();
        if let Some(name) = text.strip_prefix("# Subtest: ") {
            while open.last().is_some_and(|(d, _)| *d >= depth) {
                open.pop();
            }
            open.push((depth, name.trim().to_string()));
        } else if let Some((name, status)) = parse_result(text) {
            while open.last().is_some_and(|(d, _)| *d > depth) {
                open.pop();
            }
            let mut path: Vec<&str> = open.iter().map(|(_, n)| n.as_str()).collect();
            path.push(&name);
            let path = path.join(".");
            let worse = units.get(&path).is_none_or(|old| status > *old);
            if worse {
                units.insert(path, status);
            }
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    const KTAP: &str = "[    0.500000] Linux version 7.2.8
[    1.000000] KTAP version 1
[    1.000001] 1..1
[    1.000002]     KTAP version 1
[    1.000003]     # Subtest: example
[    1.000004]     # module: example_test
[    1.000005]     1..3
[    1.000006]     ok 1 example_simple_test
[    1.000007] random: crng init done
[    1.000008]     ok 2 example_skip_test # SKIP this test should be skipped
[    1.000009]         KTAP version 1
[    1.000010]         # Subtest: example_params_test
[    1.000011]         ok 1 example value 3
[    1.000012]         not ok 2 example value 2
[    1.000013]     # example_params_test: pass:1 fail:1 skip:0 total:2
[    1.000014]     not ok 3 example_params_test
[    1.000015] # example: pass:1 fail:1 skip:1 total:3
[    1.000016] not ok 1 example
";

    #[test]
    fn every_result_is_a_unit_named_by_its_path() {
        let units = parse(KTAP);
        let expected = [
            ("example", Status::Fail),
            ("example.example_params_test", Status::Fail),
            ("example.example_params_test.example value 2", Status::Fail),
            ("example.example_params_test.example value 3", Status::Pass),
            ("example.example_simple_test", Status::Pass),
            ("example.example_skip_test", Status::Skip),
        ];
        assert_eq!(
            units
                .iter()
                .map(|(k, v)| (k.as_str(), *v))
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn old_tap_with_tabs_and_dashes_reads_the_same() {
        let old = "TAP version 14\n1..2\n\t# Subtest: list\n\t1..1\n\tok 1 - list_test\nok 1 - list\n\t# Subtest: bits\n\tnot ok 1 - bits_test\nnot ok 2 - bits\n";
        let units = parse(old);
        assert_eq!(units["list.list_test"], Status::Pass);
        assert_eq!(units["list"], Status::Pass);
        assert_eq!(units["bits.bits_test"], Status::Fail);
        assert_eq!(units.len(), 4);
    }

    #[test]
    fn result_lines_are_strict() {
        assert_eq!(
            parse_result("ok 7 foo # skip no hardware"),
            Some(("foo".to_string(), Status::Skip))
        );
        assert_eq!(parse_result("ok then"), None);
        assert_eq!(parse_result("not ok 1"), None);
        assert_eq!(parse_result("okay 1 x"), None);
    }
}
