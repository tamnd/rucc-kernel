//! `rk probes`: the compiler probes of two builds, matched and compared.
//!
//! kbuild and Kconfig ask the compiler questions before and during a build: `cc-option`,
//! `cc-disable-warning`, `as-instr`, the `CC_HAS_` tests and the version scripts. Each one is a
//! compiler call with its output in `/dev/null` or a `.tmp_` file, which the shim records with
//! `probe` set. A probe the reference passes and rucc fails is a flag or feature kbuild will
//! silently leave out, which changes the kernel before a single unit is compiled. This module
//! lines the probes of two logs up by what they ask, so that those show.

use crate::build::error_key;
use rk_shim::record::CompileRecord;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What a probe asks, as its arguments without the compiler, the persona, the output and the
/// names of temporary files, which change from run to run.
#[must_use]
pub fn question(record: &CompileRecord) -> String {
    let mut words = Vec::new();
    let mut args = record.argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "-o" {
            args.next();
            continue;
        }
        if arg.starts_with("-fgnuc-version=") || (arg.starts_with("-o") && arg.len() > 2) {
            continue;
        }
        if arg.contains(".tmp_") {
            words.push("TMP".to_string());
            continue;
        }
        words.push(arg.clone());
    }
    words.join(" ")
}

/// A probe's answers in one log. The same question may be asked more than once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answers {
    /// Times it succeeded.
    pub yes: usize,
    /// Times it failed.
    pub no: usize,
    /// The first error of a failure, normalized.
    pub error: Option<String>,
}

impl Answers {
    /// One word for the answer: yes, no, or mixed.
    #[must_use]
    pub fn word(&self) -> &'static str {
        match (self.yes, self.no) {
            (0, 0) => "absent",
            (_, 0) => "yes",
            (0, _) => "no",
            _ => "mixed",
        }
    }
}

/// Every probe in a log by question.
#[must_use]
pub fn collect(records: &[CompileRecord]) -> BTreeMap<String, Answers> {
    let mut out: BTreeMap<String, Answers> = BTreeMap::new();
    for record in records.iter().filter(|r| r.probe) {
        let answers = out.entry(question(record)).or_default();
        if record.succeeded() {
            answers.yes += 1;
        } else {
            answers.no += 1;
            if answers.error.is_none() {
                answers.error = error_key(&record.stderr);
            }
        }
    }
    out
}

/// A question the two logs answer differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disagreement {
    /// The question.
    pub question: String,
    /// The reference's answers.
    pub reference: Answers,
    /// The other compiler's answers.
    pub other: Answers,
}

/// The questions answered differently, including those only one side asked.
#[must_use]
pub fn compare(
    reference: &BTreeMap<String, Answers>,
    other: &BTreeMap<String, Answers>,
) -> Vec<Disagreement> {
    let questions: std::collections::BTreeSet<&String> =
        reference.keys().chain(other.keys()).collect();
    questions
        .into_iter()
        .filter_map(|q| {
            let r = reference.get(q).cloned().unwrap_or_default();
            let o = other.get(q).cloned().unwrap_or_default();
            (r.word() != o.word()).then(|| Disagreement {
                question: q.clone(),
                reference: r,
                other: o,
            })
        })
        .collect()
}

/// The comparison as markdown.
#[must_use]
pub fn report(
    reference: &BTreeMap<String, Answers>,
    other: &BTreeMap<String, Answers>,
    disagreements: &[Disagreement],
) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{} probes in the reference, {} in the other, {} answered differently.",
        reference.len(),
        other.len(),
        disagreements.len()
    );
    if !disagreements.is_empty() {
        let _ = writeln!(
            s,
            "\n| reference | other | probe | other's error |\n|---|---|---|---|"
        );
        for d in disagreements {
            let _ = writeln!(
                s,
                "| {} | {} | `{}` | {} |",
                d.reference.word(),
                d.other.word(),
                d.question.replace('|', "\\|"),
                d.other
                    .error
                    .as_deref()
                    .map_or_else(String::new, |e| format!("`{}`", e.replace('|', "\\|")))
            );
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use rk_shim::record::parse_log;

    const GCC: &str = r#"{"started":1,"argv":["/o/rk-bin/rk-cc","-Werror","-mno-red-zone","-c","-x","c","/dev/null","-o",".tmp_12/tmp"],"compiler":"/g","cwd":"/o","wall-seconds":0.1,"exit":0,"probe":true}
{"started":1,"argv":["/o/rk-bin/rk-cc","-Werror","-fcf-protection=branch","-c","-x","c","/dev/null","-o",".tmp_13/tmp"],"compiler":"/g","cwd":"/o","wall-seconds":0.1,"exit":0,"probe":true}
{"started":1,"argv":["/o/rk-bin/rk-cc","-c","-o","a.o","a.c"],"compiler":"/g","cwd":"/o","wall-seconds":0.1,"exit":0}
"#;

    const RUCC: &str = r#"{"started":1,"argv":["/p/rk-bin/rk-cc","-fgnuc-version=14.2.0","-Werror","-mno-red-zone","-c","-x","c","/dev/null","-o",".tmp_40/tmp"],"compiler":"/r","cwd":"/p","wall-seconds":0.1,"exit":0,"probe":true}
{"started":1,"argv":["/p/rk-bin/rk-cc","-fgnuc-version=14.2.0","-Werror","-fcf-protection=branch","-c","-x","c","/dev/null","-o",".tmp_41/tmp"],"compiler":"/r","cwd":"/p","wall-seconds":0.1,"exit":1,"stderr":"rucc: error: unsupported option '-fcf-protection=branch'\n","probe":true}
"#;

    #[test]
    fn a_question_drops_what_changes_between_runs() {
        let (records, _) = parse_log(RUCC);
        assert_eq!(
            question(&records[0]),
            "-Werror -mno-red-zone -c -x c /dev/null"
        );
    }

    #[test]
    fn only_different_answers_are_reported() {
        let gcc = collect(&parse_log(GCC).0);
        let rucc = collect(&parse_log(RUCC).0);
        assert_eq!(gcc.len(), 2);
        let found = compare(&gcc, &rucc);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].question,
            "-Werror -fcf-protection=branch -c -x c /dev/null"
        );
        assert_eq!(found[0].other.word(), "no");
        assert_eq!(
            found[0].other.error.as_deref(),
            Some("unsupported option '_'")
        );
        assert!(
            report(&gcc, &rucc, &found)
                .starts_with("2 probes in the reference, 2 in the other, 1 answered")
        );
    }
}
