//! `rk asm-inventory`: every instruction the kernel hands to an assembler itself, counted.
//!
//! What rucc's assembler must accept beyond its own output is the kernel's `.S` files and the
//! templates of its inline asm. This module replays the reference build's units from
//! `compile.jsonl`: a C unit again with `-S`, keeping only the lines between GCC's `#APP` and
//! `#NO_APP` markers, which are the inline asm templates with their operands filled in, and an
//! assembly unit with `-E`, which is the text gas saw. Each statement is reduced to its mnemonic
//! and, on x86, the shape of its operands, and the shapes are counted by unit.

use crate::build::is_unit;
use rk_shim::record::CompileRecord;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

/// Where an instruction came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// A `.S` file.
    Assembly,
    /// An inline asm template.
    Inline,
}

/// An x86 register name reduced to its class.
fn register_class(name: &str) -> &'static str {
    const R64: &[&str] = &[
        "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "rip",
    ];
    const R32: &[&str] = &["eax", "ebx", "ecx", "edx", "esi", "edi", "ebp", "esp"];
    const R16: &[&str] = &["ax", "bx", "cx", "dx", "si", "di", "bp", "sp"];
    const R8: &[&str] = &[
        "al", "bl", "cl", "dl", "ah", "bh", "ch", "dh", "sil", "dil", "bpl", "spl",
    ];
    const SEG: &[&str] = &["cs", "ds", "es", "fs", "gs", "ss"];
    let numbered = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    let r = |suffix: &str| {
        name.strip_prefix('r')
            .and_then(|n| n.strip_suffix(suffix))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    if R64.contains(&name) || numbered("r") {
        "r64"
    } else if R32.contains(&name) || r("d") {
        "r32"
    } else if R16.contains(&name) || r("w") {
        "r16"
    } else if R8.contains(&name) || r("b") {
        "r8"
    } else if SEG.contains(&name) {
        "seg"
    } else if numbered("cr") {
        "cr"
    } else if numbered("dr") || numbered("db") {
        "dr"
    } else if numbered("xmm") {
        "xmm"
    } else if numbered("ymm") {
        "ymm"
    } else if numbered("zmm") {
        "zmm"
    } else if numbered("k") {
        "k"
    } else if numbered("mm") {
        "mm"
    } else if name.starts_with("st") {
        "st"
    } else {
        "reg"
    }
}

/// Instruction prefixes, which may stand alone as `lock;` before the instruction they modify.
const PREFIXES: &[&str] = &[
    "lock", "rep", "repe", "repz", "repne", "repnz", "notrack", "ds", "cs",
];

/// The shape of one AT&T operand. A macro argument, `\\name`, is `\\arg`.
#[must_use]
pub fn operand_shape(operand: &str) -> String {
    let operand = operand.trim();
    if operand.starts_with('\\') {
        return "\\arg".to_string();
    }
    if let Some(rest) = operand.strip_prefix('*') {
        return format!("*{}", operand_shape(rest));
    }
    if let Some(imm) = operand.strip_prefix('$') {
        let numeric = imm
            .trim_start_matches(['-', '(', '~'])
            .starts_with(|c: char| c.is_ascii_digit());
        return if numeric { "$imm" } else { "$sym" }.to_string();
    }
    if let Some(reg) = operand.strip_prefix('%') {
        if let Some((seg, rest)) = reg.split_once(':') {
            return format!("%{}:{}", register_class(seg), operand_shape(rest));
        }
        return format!("%{}", register_class(reg));
    }
    if operand.contains('(') {
        let base_index = operand
            .rsplit_once('(')
            .map_or("", |(_, r)| r.trim_end_matches(')'));
        let parts = base_index
            .split(',')
            .filter(|p| !p.trim().is_empty())
            .count();
        let displacement = operand.split('(').next().unwrap_or("").trim();
        let disp = if displacement.is_empty() {
            ""
        } else if displacement.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
            "N"
        } else {
            "sym"
        };
        return match parts {
            0 | 1 => format!("{disp}(b)"),
            _ => format!("{disp}(b,i,s)"),
        };
    }
    if operand.starts_with(|c: char| c.is_ascii_digit()) {
        "N".to_string()
    } else {
        "sym".to_string()
    }
}

/// Split operands at top level commas, not the ones inside brackets.
fn split_operands(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if !text[start..].trim().is_empty() {
        out.push(&text[start..]);
    }
    out
}

/// The statements of assembly text as `mnemonic shape` strings. Directives, labels and the names
/// of macros the text defines are left out. `x86` switches on `#` comments and operand shapes.
#[must_use]
pub fn statements(text: &str, x86: bool) -> Vec<String> {
    let macros: BTreeSet<&str> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix(".macro"))
        .filter_map(|rest| {
            rest.split(|c: char| c.is_whitespace() || c == ',')
                .find(|w| !w.is_empty())
        })
        .collect();
    let mut out = Vec::new();
    let mut pending = String::new();
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("");
        let line = if x86 {
            line.split('#').next().unwrap_or("")
        } else {
            line
        };
        for statement in line.split(';') {
            let mut statement = statement.trim();
            while let Some((label, rest)) = statement.split_once(':') {
                let is_label = !label.is_empty()
                    && label
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_.$".contains(c));
                if !is_label || rest.starts_with(':') {
                    break;
                }
                statement = rest.trim();
            }
            if statement.is_empty() || statement.starts_with('.') || statement.starts_with('\\') {
                continue;
            }
            let (mut mnemonic, mut rest) = statement
                .split_once(char::is_whitespace)
                .map_or((statement, ""), |(m, r)| (m, r.trim()));
            if PREFIXES.contains(&mnemonic) && rest.is_empty() {
                pending.push_str(mnemonic);
                pending.push(' ');
                continue;
            }
            let mut prefix = std::mem::take(&mut pending);
            while PREFIXES.contains(&mnemonic) && !rest.is_empty() {
                prefix.push_str(mnemonic);
                prefix.push(' ');
                (mnemonic, rest) = rest
                    .split_once(char::is_whitespace)
                    .map_or((rest, ""), |(m, r)| (m, r.trim()));
            }
            if macros.contains(mnemonic)
                || mnemonic.contains(['=', '(', '"'])
                || !mnemonic.starts_with(|c: char| c.is_ascii_alphabetic())
            {
                continue;
            }
            let mnemonic = mnemonic.to_ascii_lowercase();
            if x86 && !rest.is_empty() {
                let shapes: Vec<String> = split_operands(rest)
                    .into_iter()
                    .map(operand_shape)
                    .collect();
                out.push(format!("{prefix}{mnemonic} {}", shapes.join(",")));
            } else {
                out.push(format!("{prefix}{mnemonic}"));
            }
        }
    }
    out
}

/// The inline asm of `-S` output: the lines between `#APP` and `#NO_APP`.
#[must_use]
pub fn inline_asm(text: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "#APP" || trimmed == "// #APP" || trimmed == "@APP" {
            inside = true;
        } else if trimmed == "#NO_APP" || trimmed == "// #NO_APP" || trimmed == "@NO_APP" {
            inside = false;
        } else if inside && !trimmed.starts_with("# ") {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The command that replays a unit to text: the real compiler, the same arguments, `-S` for C
/// and `-E` for assembly, output to standard out, and no dependency file.
#[must_use]
pub fn replay_args(record: &CompileRecord) -> Option<(Source, Vec<String>)> {
    let input = record.inputs.iter().find_map(|i| {
        let ext = Path::new(&i.path).extension()?.to_str()?;
        (ext == "c" || ext == "S").then_some(ext)
    })?;
    let source = if input == "S" {
        Source::Assembly
    } else {
        Source::Inline
    };
    let mut args = Vec::new();
    let mut words = record.argv.iter().skip(1);
    while let Some(word) = words.next() {
        if word == "-o" {
            words.next();
            continue;
        }
        if word == "-c" || word.starts_with("-Wp,-MMD") || word.starts_with("-Wp,-MD") {
            continue;
        }
        args.push(word.clone());
    }
    args.push(
        if source == Source::Assembly {
            "-E"
        } else {
            "-S"
        }
        .to_string(),
    );
    args.extend(["-o".to_string(), "-".to_string()]);
    Some((source, args))
}

/// The inventory: every statement shape, with the units it appears in.
#[derive(Debug, Default)]
pub struct Inventory {
    /// Shape to source and unit names.
    pub shapes: BTreeMap<String, (BTreeSet<Source>, BTreeSet<String>)>,
    /// Units replayed.
    pub units: usize,
    /// Units that could not be replayed, with the reason.
    pub failed: Vec<(String, String)>,
}

impl Inventory {
    /// Add the statements of one unit.
    pub fn add(&mut self, unit: &str, source: Source, statements: &[String]) {
        self.units += 1;
        for statement in statements {
            let entry = self.shapes.entry(statement.clone()).or_default();
            entry.0.insert(source);
            entry.1.insert(unit.to_string());
        }
    }

    /// The mnemonics, with how many units use each.
    #[must_use]
    pub fn mnemonics(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (shape, (_, units)) in &self.shapes {
            let mnemonic = shape.split(' ').find(|w| !PREFIXES.contains(w));
            out.entry(mnemonic.unwrap_or(shape).to_string())
                .or_default()
                .extend(units.iter().cloned());
        }
        out
    }

    /// The inventory as markdown: mnemonics by use, then every shape.
    #[must_use]
    pub fn report(&self) -> String {
        let mut s = String::new();
        let mnemonics = self.mnemonics();
        let _ = writeln!(
            s,
            "{} units replayed, {} could not be. {} mnemonics in {} shapes.",
            self.units,
            self.failed.len(),
            mnemonics.len(),
            self.shapes.len()
        );
        let mut ranked: Vec<(&String, usize)> =
            mnemonics.iter().map(|(m, u)| (m, u.len())).collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let _ = writeln!(s, "\n## Mnemonics\n\n| mnemonic | units |\n|---|---|");
        for (mnemonic, units) in ranked {
            let _ = writeln!(s, "| `{mnemonic}` | {units} |");
        }
        let _ = writeln!(
            s,
            "\n## Shapes\n\n| shape | from | units | example |\n|---|---|---|---|"
        );
        for (shape, (sources, units)) in &self.shapes {
            let from: Vec<&str> = sources
                .iter()
                .map(|s| match s {
                    Source::Assembly => ".S",
                    Source::Inline => "inline",
                })
                .collect();
            let _ = writeln!(
                s,
                "| `{}` | {} | {} | `{}` |",
                shape.replace('|', "\\|"),
                from.join(" "),
                units.len(),
                units.iter().next().map_or("", String::as_str)
            );
        }
        if !self.failed.is_empty() {
            let _ = writeln!(s, "\n## Not replayed\n\n| unit | why |\n|---|---|");
            for (unit, why) in &self.failed {
                let _ = writeln!(s, "| `{unit}` | {} |", why.replace('|', "\\|"));
            }
        }
        s
    }
}

/// Replay every successful unit of a log and build the inventory, `jobs` units at a time.
pub fn collect(records: &[CompileRecord], tree: &Path, x86: bool, jobs: usize) -> Inventory {
    let work: Vec<&CompileRecord> = records
        .iter()
        .filter(|r| is_unit(r) && r.succeeded() && r.delegated.is_none())
        .collect();
    let next = Mutex::new(work.into_iter());
    let inventory = Mutex::new(Inventory::default());
    std::thread::scope(|scope| {
        for _ in 0..jobs.max(1) {
            scope.spawn(|| {
                while let Some(record) = next.lock().ok().and_then(|mut it| it.next()) {
                    let Some((source, args)) = replay_args(record) else {
                        continue;
                    };
                    let unit = record
                        .inputs
                        .iter()
                        .map(|i| Path::new(&i.path))
                        .find(|p| p.extension().is_some_and(|e| e == "c" || e == "S"))
                        .map(|p| {
                            let full = Path::new(&record.cwd).join(p);
                            full.strip_prefix(tree).map_or_else(
                                |_| p.display().to_string(),
                                |r| r.display().to_string(),
                            )
                        })
                        .unwrap_or_default();
                    let output = Command::new(&record.compiler)
                        .args(&args)
                        .current_dir(&record.cwd)
                        .envs(&record.env)
                        .output();
                    let mut inv = match inventory.lock() {
                        Ok(inv) => inv,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    match output {
                        Ok(out) if out.status.success() => {
                            let text = String::from_utf8_lossy(&out.stdout);
                            let text = match source {
                                Source::Inline => inline_asm(&text),
                                Source::Assembly => text.into_owned(),
                            };
                            inv.add(&unit, source, &statements(&text, x86));
                        }
                        Ok(out) => {
                            let why = String::from_utf8_lossy(&out.stderr)
                                .lines()
                                .find(|l| l.contains("error"))
                                .unwrap_or("failed")
                                .to_string();
                            inv.failed.push((unit, why));
                        }
                        Err(e) => inv.failed.push((unit, e.to_string())),
                    }
                }
            });
        }
    });
    let mut inventory = inventory.into_inner().unwrap_or_default();
    inventory.failed.sort();
    inventory
}

#[cfg(test)]
mod tests {
    use super::*;
    use rk_shim::record::parse_log;

    #[test]
    fn operands_reduce_to_shapes() {
        assert_eq!(operand_shape("%rax"), "%r64");
        assert_eq!(operand_shape("%r12d"), "%r32");
        assert_eq!(operand_shape("%cr4"), "%cr");
        assert_eq!(operand_shape("$0x10"), "$imm");
        assert_eq!(operand_shape("$pcpu_hot"), "$sym");
        assert_eq!(operand_shape("%gs:pcpu_hot+16"), "%seg:sym");
        assert_eq!(operand_shape("%gs:0x28(%rip)"), "%seg:N(b)");
        assert_eq!(operand_shape("8(%rsp)"), "N(b)");
        assert_eq!(operand_shape("sys_call_table(,%rax,8)"), "sym(b,i,s)");
        assert_eq!(operand_shape("*%rax"), "*%r64");
    }

    #[test]
    fn statements_skip_labels_directives_comments_and_macros() {
        let text = "\
.macro PUSH_REGS rdx=%rdx
    pushq \\rdx
.endm
SYM_CODE_START(entry)
1:  PUSH_REGS
    lock; cmpxchgq %rdx, (%rdi)   # the swap
    movq %cr4, %rax ; wrmsr
    .pushsection .altinstructions, \"a\"
    ljmp $0x10, $2f
";
        assert_eq!(
            statements(text, true),
            [
                "pushq \\arg",
                "lock cmpxchgq %r64,(b)",
                "movq %cr,%r64",
                "wrmsr",
                "ljmp $imm,$imm"
            ]
        );
    }

    #[test]
    fn inline_asm_is_what_lies_between_the_markers() {
        let text = "\tmovl $1, %eax\n#APP\n# 12 \"x.h\" 1\n\tstac\n#NO_APP\n\tret\n";
        assert_eq!(inline_asm(text), "\tstac\n");
    }

    #[test]
    fn a_unit_replays_without_its_output_or_dependency_file() {
        let log = r#"{"started":1,"argv":["/o/rk-bin/rk-cc","-Wp,-MMD,kernel/.fork.o.d","-O2","-c","-o","kernel/fork.o","/s/kernel/fork.c"],"compiler":"/usr/bin/gcc-14","cwd":"/o","inputs":[{"path":"/s/kernel/fork.c","sha256":"a"}],"wall-seconds":0.1,"exit":0}"#;
        let (records, _) = parse_log(log);
        let (source, args) = replay_args(&records[0]).unwrap();
        assert_eq!(source, Source::Inline);
        assert_eq!(args, ["-O2", "/s/kernel/fork.c", "-S", "-o", "-"]);
    }

    #[test]
    fn the_inventory_counts_units_per_mnemonic() {
        let mut inv = Inventory::default();
        inv.add("a.S", Source::Assembly, &["movq %cr,%r64".to_string()]);
        inv.add("b.c", Source::Inline, &["movq %r64,%cr".to_string()]);
        assert_eq!(inv.mnemonics()["movq"].len(), 2);
        assert!(
            inv.report()
                .starts_with("2 units replayed, 0 could not be. 1 mnemonics in 2 shapes.")
        );
    }
}
