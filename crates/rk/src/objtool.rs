//! `rk objtool-report`: objtool's warnings for two builds, bucketed and compared.
//!
//! objtool reads the machine code of every x86 object and checks what the kernel assumes of it:
//! frame pointer setup before a call, no fall through from one function into the next, no
//! unreachable code, stack state that agrees at every join, and more. GCC's output is what it was
//! written against, so rucc's first builds are expected to trip it in ways no other test sees
//! (open question 2 of the plan). kbuild runs the tree's own objtool, which is the pinned one,
//! on every object when `CONFIG_OBJTOOL` or `CONFIG_STACK_VALIDATION` is set, and it prints to
//! the build log, so this reads `build.log` and needs no objects.
//!
//! Each warning is reduced to its message with the function names and offsets taken out, and to
//! the shape of the function it is about: a plain function, or one of GCC's `.cold`, `.isra`,
//! `.constprop` or `.part` clones. The buckets are counted on both sides, and a bucket the other
//! build has more of than the reference is a finding.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// One objtool warning, reduced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// The object objtool was run on.
    pub object: String,
    /// The function, when the warning names one.
    pub function: String,
    /// The message with names and offsets replaced.
    pub message: String,
}

/// The shape of a function name: `plain`, a GCC clone suffix, or `section` for an offset into a
/// section with no function.
#[must_use]
pub fn shape(function: &str) -> &'static str {
    if function.is_empty() {
        return "none";
    }
    if function.starts_with('.') {
        return "section";
    }
    for suffix in ["cold", "isra", "constprop", "part", "llvm"] {
        if function.contains(&format!(".{suffix}")) {
            return suffix;
        }
    }
    "plain"
}

/// Replace every `name()` with `FN()`, every `name+0x1f` with `FN+N` and every other hex number
/// with `N`.
fn normalize(message: &str) -> String {
    message
        .split(' ')
        .map(|word| {
            let (word, tail) = match word.find([',', ':', ';']) {
                Some(i) if i + 1 == word.len() => word.split_at(i),
                _ => (word, ""),
            };
            let out = if let Some((_, rest)) = word.split_once("()") {
                if word.starts_with("()") {
                    word.to_string()
                } else {
                    format!("FN(){}", if rest.contains("+0x") { "+N" } else { "" })
                }
            } else if word.contains("+0x") {
                "FN+N".to_string()
            } else if word.starts_with("0x") {
                "N".to_string()
            } else {
                word.to_string()
            };
            format!("{out}{tail}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The function a location names: `foo()+0x1a`, `foo+0x1a` or `foo()`.
fn function_of(location: &str) -> String {
    let name = location.split("+0x").next().unwrap_or(location);
    name.strip_suffix("()").unwrap_or(name).to_string()
}

/// Read one line of a build log, if it is an objtool warning or error.
#[must_use]
pub fn parse_line(line: &str) -> Option<Warning> {
    let (object, rest) = line
        .split_once(": warning: objtool: ")
        .or_else(|| line.split_once(": error: objtool: "))?;
    let object = object.trim().to_string();
    let rest = rest.trim();
    let first = rest.split(' ').next().unwrap_or_default();
    let (function, message) = if let Some(location) = first.strip_suffix(':') {
        (
            function_of(location),
            rest[first.len()..].trim().to_string(),
        )
    } else if first.contains("()") {
        (function_of(first), rest[first.len()..].trim().to_string())
    } else {
        (String::new(), rest.to_string())
    };
    Some(Warning {
        object,
        function,
        message: normalize(&message),
    })
}

/// Every objtool warning in a build log.
#[must_use]
pub fn parse_log(text: &str) -> Vec<Warning> {
    text.lines().filter_map(parse_line).collect()
}

/// Read the warnings of a build directory's `build.log`, or of a log file.
pub fn load(path: &Path) -> Result<Vec<Warning>, String> {
    let file = if path.is_dir() {
        path.join("build.log")
    } else {
        path.to_path_buf()
    };
    let text =
        std::fs::read_to_string(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
    Ok(parse_log(&text))
}

/// One bucket: its count on each side and an example from the other side, or from the
/// reference when the other has none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bucket {
    /// Warnings in the reference.
    pub reference: usize,
    /// Warnings in the other build.
    pub other: usize,
    /// An object and function it came from.
    pub example: String,
}

/// Bucket both sides by message and function shape.
#[must_use]
pub fn bucket(reference: &[Warning], other: &[Warning]) -> BTreeMap<(String, String), Bucket> {
    let mut out: BTreeMap<(String, String), Bucket> = BTreeMap::new();
    for (side, warnings) in [(0, reference), (1, other)] {
        for w in warnings {
            let b = out
                .entry((w.message.clone(), shape(&w.function).to_string()))
                .or_default();
            if side == 0 {
                b.reference += 1;
            } else {
                b.other += 1;
            }
            if b.example.is_empty() || (side == 1 && b.other == 1) {
                b.example = if w.function.is_empty() {
                    w.object.clone()
                } else {
                    format!("{} {}", w.object, w.function)
                };
            }
        }
    }
    out
}

/// Whether the other build has no bucket with more warnings than the reference.
#[must_use]
pub fn clean(buckets: &BTreeMap<(String, String), Bucket>) -> bool {
    buckets.values().all(|b| b.other <= b.reference)
}

/// The buckets as markdown, the largest excess first.
#[must_use]
pub fn report(buckets: &BTreeMap<(String, String), Bucket>) -> String {
    let mut s = String::from("### objtool\n\n");
    let (r, o): (usize, usize) = buckets
        .values()
        .fold((0, 0), |(r, o), b| (r + b.reference, o + b.other));
    let _ = writeln!(
        s,
        "{r} warnings in the reference and {o} in the other build, in {} buckets.\n",
        buckets.len()
    );
    if buckets.is_empty() {
        return s;
    }
    let mut rows: Vec<(&(String, String), &Bucket)> = buckets.iter().collect();
    rows.sort_by_key(|(_, b)| std::cmp::Reverse(b.other.cast_signed() - b.reference.cast_signed()));
    s.push_str("| message | function | reference | other | example |\n|---|---|---|---|---|\n");
    for ((message, shape), b) in rows.iter().take(80) {
        let _ = writeln!(
            s,
            "| {} | {shape} | {} | {} | {} |",
            message.replace('|', "\\|"),
            b.reference,
            b.other,
            b.example
        );
    }
    if rows.len() > 80 {
        let _ = writeln!(s, "\nand {} more buckets.", rows.len() - 80);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "  CC      kernel/fork.o
kernel/fork.o: warning: objtool: copy_process+0x1a2: unreachable instruction
vmlinux.o: warning: objtool: do_idle.cold()+0x10: call without frame pointer save/setup
mm/slub.o: warning: objtool: kfree() falls through to next function kmalloc()
fs/x.o: warning: objtool: .text+0x40: unreachable instruction
fs/y.o: error: objtool: bar.isra.0+0x3: sibling call from callable instruction with modified stack frame
";

    #[test]
    fn warnings_reduce_to_message_and_shape() {
        let w = parse_log(LOG);
        assert_eq!(w.len(), 5);
        assert_eq!(w[0].function, "copy_process");
        assert_eq!(w[0].message, "unreachable instruction");
        assert_eq!(shape(&w[1].function), "cold");
        assert_eq!(w[2].function, "kfree");
        assert_eq!(w[2].message, "falls through to next function FN()");
        assert_eq!(shape(&w[3].function), "section");
        assert_eq!(shape(&w[4].function), "isra");
        assert_eq!(
            normalize("call to __ubsan_handle_x() with UACCESS enabled"),
            "call to FN() with UACCESS enabled"
        );
    }

    #[test]
    fn a_bucket_the_other_build_has_more_of_is_a_finding() {
        let reference = parse_log(LOG);
        let other = parse_log(&format!(
            "{LOG}a/b.o: warning: objtool: f+0x2: unreachable instruction\n"
        ));
        let buckets = bucket(&reference, &other);
        assert!(!clean(&buckets));
        assert!(clean(&bucket(&other, &reference)));
        let text = report(&buckets);
        assert!(
            text.contains(
                "| unreachable instruction | plain | 1 | 2 | kernel/fork.o copy_process |"
            )
        );
    }
}
