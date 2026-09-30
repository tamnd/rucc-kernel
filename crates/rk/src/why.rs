//! `rk config-diff --why`: the Kconfig expressions and compiler probes behind each difference.
//!
//! For every symbol that differs, this reads what Kconfig says decides it (`depends on`, `def_bool`
//! and `def_tristate`, `default ... if`, the `if` blocks around it and the `select` lines that
//! name it) and follows the symbols those name, a few levels deep, collecting the calls to
//! `$(cc-option,...)`, `$(as-instr,...)`, `$(success,...)` and the rest. Each call is matched to
//! the probes the two builds answered differently. A difference whose own expressions lead to such
//! a probe is a root. One that only depends on another differing symbol follows from it, so the
//! report can lead with the few causes rather than their consequences (plan 5.2).
//!
//! The reader is a heuristic, not Kconfig. It does not expand macros, so a probe hidden behind a
//! macro of a macro is matched only by the flags written in the call itself. It keeps every
//! definition of a symbol it finds in the tree, apart from other architectures' directories, and
//! reads `menu` blocks and `choice` groups as if they were not there. A call matches a probe when
//! the probe's question contains every flag the call names, or for `as-instr` and the other calls
//! that feed standard input, when the input contains the call's text. That is loose on purpose: a
//! false match shows up as a probe next to a symbol, and a missed one as a symbol with no reason.

use crate::kconfig::Difference;
use crate::probes::Disagreement;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// What decides one symbol, as the expressions Kconfig gives for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// Each expression, with the keyword it came from, as `depends on CC_HAS_X`.
    pub expressions: Vec<String>,
}

/// Every symbol's entry in a tree.
pub type Symbols = BTreeMap<String, Entry>;

fn indent(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 8 } else { 1 })
        .sum()
}

/// Read one Kconfig file into `symbols`.
pub fn parse_into(text: &str, symbols: &mut Symbols) {
    // Lines ending in a backslash go on.
    let text = text.replace("\\\n", " ");
    let mut current: Option<String> = None;
    let mut ifs: Vec<String> = Vec::new();
    let mut help: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        // Help text is everything indented deeper than its keyword, blank lines included.
        if let Some(at) = help {
            if trimmed.is_empty() || indent(line) > at {
                continue;
            }
            help = None;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (word, rest) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(w, r)| (w, r.trim()));
        match word {
            "config" | "menuconfig" => {
                let symbol = rest.to_string();
                let entry = symbols.entry(symbol.clone()).or_default();
                for condition in &ifs {
                    push(entry, format!("if {condition}"));
                }
                current = Some(symbol);
            }
            "if" => {
                ifs.push(rest.to_string());
                current = None;
            }
            "endif" => {
                ifs.pop();
                current = None;
            }
            "choice" | "endchoice" | "menu" | "endmenu" | "source" | "comment" | "mainmenu" => {
                current = None;
            }
            "help" | "---help---" => help = Some(indent(line)),
            _ => {
                let Some(symbol) = &current else {
                    continue;
                };
                if let Some(expr) = rest.strip_prefix("on ").filter(|_| word == "depends") {
                    push(
                        symbols.entry(symbol.clone()).or_default(),
                        format!("depends on {}", expr.trim()),
                    );
                } else if matches!(word, "def_bool" | "def_tristate" | "default") {
                    push(
                        symbols.entry(symbol.clone()).or_default(),
                        format!("{word} {rest}"),
                    );
                } else if matches!(word, "select" | "imply") {
                    // `select X if Y` makes X depend on this symbol, and on Y.
                    let (target, condition) = rest
                        .split_once(" if ")
                        .map_or((rest, None), |(t, c)| (t.trim(), Some(c.trim())));
                    let by = match condition {
                        Some(c) => format!("selected by {symbol} if {c}"),
                        None => format!("selected by {symbol}"),
                    };
                    push(symbols.entry(target.to_string()).or_default(), by);
                } else if matches!(word, "bool" | "tristate")
                    && let Some((_, condition)) = rest.split_once(" if ")
                {
                    push(
                        symbols.entry(symbol.clone()).or_default(),
                        format!("visible if {}", condition.trim()),
                    );
                }
            }
        }
    }
}

fn push(entry: &mut Entry, expression: String) {
    if !entry.expressions.contains(&expression) {
        entry.expressions.push(expression);
    }
}

/// Read every `Kconfig*` file in a tree. Under `arch/` only `srcarch`'s directory is read, since
/// the other architectures define many of the same symbols differently.
pub fn load(tree: &Path, srcarch: &str) -> Result<Symbols, String> {
    let mut symbols = Symbols::new();
    let mut dirs = vec![tree.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if kind.is_dir() {
                let other_arch = dir.file_name().is_some_and(|d| d == "arch")
                    && dir.parent() == Some(tree)
                    && name != srcarch;
                if !other_arch && !name.starts_with('.') {
                    dirs.push(path);
                }
            } else if kind.is_file()
                && name.starts_with("Kconfig")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                parse_into(&text, &mut symbols);
            }
        }
    }
    Ok(symbols)
}

/// The `$(name,args)` calls in an expression, outermost first, with nested calls kept as text in
/// their parent's arguments and also listed on their own.
#[must_use]
pub fn calls(expression: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes: Vec<char> = expression.chars().collect();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == '$' && bytes[i + 1] == '(' {
            let mut depth = 0;
            let mut end = None;
            for (j, c) in bytes.iter().enumerate().skip(i + 1) {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(j);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else {
                break;
            };
            let inner: String = bytes[i + 2..end].iter().collect();
            let (name, args) = inner.split_once(',').unwrap_or((inner.as_str(), ""));
            out.push((name.trim().to_string(), args.trim().to_string()));
            out.extend(calls(args));
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// The symbols an expression names: words in capitals, digits and underscores, outside calls.
#[must_use]
pub fn symbols_in(expression: &str) -> Vec<String> {
    let mut text = expression.to_string();
    // Calls go first, since their arguments are flags and not symbols.
    for (name, args) in calls(expression) {
        text = text.replace(&format!("$({name},{args})"), " ");
        text = text.replace(&format!("$({name})"), " ");
    }
    let mut out = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        let symbol = word.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && word
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        let keyword = matches!(word, "if" | "on" | "y" | "n" | "m");
        if symbol && !keyword && !out.contains(&word.to_string()) {
            out.push(word.to_string());
        }
    }
    out
}

/// Whether a call asks what a probe asked.
#[must_use]
pub fn matches(name: &str, args: &str, question: &str) -> bool {
    let (words, input) = question
        .split_once(" <<< ")
        .map_or((question, ""), |(w, i)| (w, i));
    if name.starts_with("as-instr") {
        // as-instr prints its first argument with printf %b, so \n starts a new line, and the
        // Kconfig writes a comma inside it as $(comma).
        let text = args
            .split(',')
            .next()
            .unwrap_or_default()
            .replace("$(comma)", ",")
            .replace("\\n", "; ")
            .trim()
            .to_string();
        return !text.is_empty() && input.contains(&text);
    }
    let flags: Vec<&str> = args
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        // The output is not part of a question, so -o is not looked for.
        .filter(|w| w.starts_with('-') && w.len() > 1 && *w != "-o")
        .collect();
    let asked: BTreeSet<&str> = words.split_whitespace().collect();
    !flags.is_empty() && flags.iter().all(|f| asked.contains(f))
}

/// A difference explained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Why {
    /// The symbol.
    pub symbol: String,
    /// Its own expressions.
    pub expressions: Vec<String>,
    /// The differing probes found through it: the symbol whose expression made the call, the call
    /// and the probe.
    pub probes: Vec<(String, String, String)>,
    /// Other differing symbols it depends on.
    pub follows: Vec<String>,
}

impl Why {
    /// Whether the symbol's own expressions lead to a differing probe.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.probes.iter().any(|(by, _, _)| *by == self.symbol)
    }
}

/// How many symbols deep to follow.
const DEPTH: usize = 3;

/// Explain every difference.
#[must_use]
pub fn explain(
    differences: &[Difference],
    symbols: &Symbols,
    disagreements: &[Disagreement],
) -> Vec<Why> {
    let differing: BTreeSet<&str> = differences.iter().map(|d| d.symbol.as_str()).collect();
    differences
        .iter()
        .map(|d| {
            let expressions = symbols
                .get(&d.symbol)
                .map(|e| e.expressions.clone())
                .unwrap_or_default();
            let mut why = Why {
                symbol: d.symbol.clone(),
                expressions,
                probes: Vec::new(),
                follows: Vec::new(),
            };
            let mut seen = BTreeSet::from([d.symbol.clone()]);
            let mut level = vec![d.symbol.clone()];
            for _ in 0..DEPTH {
                let mut next = Vec::new();
                for symbol in &level {
                    let Some(entry) = symbols.get(symbol) else {
                        continue;
                    };
                    for expression in &entry.expressions {
                        for (name, args) in calls(expression) {
                            for q in disagreements {
                                if matches(&name, &args, &q.question) {
                                    let found = (
                                        symbol.clone(),
                                        format!("$({name},{args})"),
                                        q.question.clone(),
                                    );
                                    if !why.probes.contains(&found) {
                                        why.probes.push(found);
                                    }
                                }
                            }
                        }
                        for s in symbols_in(expression) {
                            if differing.contains(s.as_str())
                                && s != d.symbol
                                && !why.follows.contains(&s)
                            {
                                why.follows.push(s.clone());
                            }
                            if seen.insert(s.clone()) {
                                next.push(s);
                            }
                        }
                    }
                }
                level = next;
            }
            why
        })
        .collect()
}

/// The explanations as markdown, roots first.
#[must_use]
pub fn report(whys: &[Why], disagreements: &[Disagreement]) -> String {
    let answers: BTreeMap<&str, &Disagreement> = disagreements
        .iter()
        .map(|d| (d.question.as_str(), d))
        .collect();
    let (roots, rest): (Vec<&Why>, Vec<&Why>) = whys.iter().partition(|w| w.is_root());
    let mut s = format!(
        "\nWhy: {} roots, whose own probes answer differently, and {} others.\n\n| symbol | decided by | probe | reference | other |\n|---|---|---|---|---|\n",
        roots.len(),
        rest.len()
    );
    for w in roots.iter().chain(rest.iter()) {
        let decided = if w.expressions.is_empty() {
            "not found in Kconfig".to_string()
        } else {
            w.expressions.join("; ")
        };
        let decided = match w.follows.as_slice() {
            [] => decided,
            f => format!("{decided} (follows {})", f.join(", ")),
        };
        match w.probes.first() {
            Some((by, call, question)) => {
                let d = answers.get(question.as_str());
                let _ = writeln!(
                    s,
                    "| {} | {} | {}`{}` | {} | {} |",
                    w.symbol,
                    escape(&decided),
                    if *by == w.symbol {
                        String::new()
                    } else {
                        format!("through {by}: ")
                    },
                    escape(&format!("{call} asks {question}")),
                    d.map_or("", |d| d.reference.word()),
                    d.map_or("", |d| d.other.word())
                );
            }
            None => {
                let _ = writeln!(s, "| {} | {} | | | |", w.symbol, escape(&decided));
            }
        }
    }
    s
}

fn escape(text: &str) -> String {
    text.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probes::Answers;

    const KCONFIG: &str = r#"
config CC_HAS_NAMED_AS
	def_bool $(success,echo 'int __seg_fs fs; int __seg_gs gs;' | $(CC) -x c - -S -o /dev/null)
	depends on CC_IS_GCC

config USE_X86_SEG_SUPPORT
	def_bool y
	depends on CC_HAS_NAMED_AS
	help
	  depends on NOTHING_HERE is help text, not a dependency.

if X86_64
config CC_HAS_RETURN_THUNK
	def_bool $(cc-option,-mfunction-return=thunk-extern)
endif

config AS_WRUSS
	def_bool $(as-instr64,wrussq %rax$(comma)(%rbx))

config STACKPROTECTOR
	bool "Stack Protector buffer overflow detection"
	depends on HAVE_STACKPROTECTOR
	depends on $(cc-option,-fstack-protector)
	select X86_HELPER if X86_64
"#;

    fn disagreement(question: &str) -> Disagreement {
        Disagreement {
            question: question.to_string(),
            reference: Answers {
                yes: 1,
                ..Answers::default()
            },
            other: Answers {
                no: 1,
                ..Answers::default()
            },
        }
    }

    fn difference(symbol: &str) -> Difference {
        Difference {
            symbol: symbol.to_string(),
            reference: Some("y".to_string()),
            other: None,
            reason: None,
        }
    }

    #[test]
    fn entries_keep_their_expressions_and_skip_help_text() {
        let mut symbols = Symbols::new();
        parse_into(KCONFIG, &mut symbols);
        assert_eq!(
            symbols["USE_X86_SEG_SUPPORT"].expressions,
            ["def_bool y", "depends on CC_HAS_NAMED_AS"]
        );
        assert_eq!(
            symbols["CC_HAS_RETURN_THUNK"].expressions,
            [
                "if X86_64",
                "def_bool $(cc-option,-mfunction-return=thunk-extern)"
            ]
        );
        assert_eq!(
            symbols["X86_HELPER"].expressions,
            ["selected by STACKPROTECTOR if X86_64"]
        );
        assert!(!symbols.contains_key("NOTHING_HERE"));
    }

    #[test]
    fn calls_and_symbols_are_told_apart() {
        let found = calls("$(success,$(CC) -mx -c) && FOO");
        assert_eq!(found[0].0, "success");
        assert_eq!(found[1], ("CC".to_string(), String::new()));
        assert_eq!(
            symbols_in("depends on $(cc-option,-mno-sse) && (X86_64 || !CC_IS_CLANG)"),
            ["X86_64", "CC_IS_CLANG"]
        );
    }

    #[test]
    fn a_call_matches_the_probe_that_asked_it() {
        assert!(matches(
            "cc-option",
            "-mfunction-return=thunk-extern",
            "-Werror -mfunction-return=thunk-extern -c -x c /dev/null"
        ));
        assert!(!matches(
            "cc-option",
            "-mfunction-return=thunk-extern",
            "-Werror -mno-red-zone -c -x c /dev/null"
        ));
        assert!(matches(
            "as-instr64",
            "wrussq %rax$(comma)(%rbx)",
            "-c -x assembler-with-cpp - <<< wrussq %rax,(%rbx)"
        ));
        assert!(!matches(
            "as-instr64",
            "wrussq %rax$(comma)(%rbx)",
            "-c -x assembler-with-cpp - <<< endbr64"
        ));
    }

    #[test]
    fn a_root_leads_and_its_consequence_follows_it() {
        let mut symbols = Symbols::new();
        parse_into(KCONFIG, &mut symbols);
        let differences = [
            difference("STACKPROTECTOR"),
            difference("X86_HELPER"),
            difference("CC_VERSION_TEXT"),
        ];
        let disagreements = [disagreement("-Werror -fstack-protector -c -x c /dev/null")];
        let whys = explain(&differences, &symbols, &disagreements);
        assert!(whys[0].is_root());
        assert!(!whys[1].is_root());
        assert_eq!(whys[1].follows, ["STACKPROTECTOR"]);
        assert_eq!(whys[1].probes[0].0, "STACKPROTECTOR");
        assert!(whys[2].expressions.is_empty());
        let text = report(&whys, &disagreements);
        assert!(text.contains("1 roots"));
        assert!(text.contains("| STACKPROTECTOR | "));
        assert!(text.contains("| yes | no |"));
        assert!(text.contains("not found in Kconfig"));
    }
}
