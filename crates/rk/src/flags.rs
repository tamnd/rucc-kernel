//! `rk flags-diff`: the command line of every unit in two builds, compared.
//!
//! kbuild's `cc-option` drops a flag the compiler refuses without a word, so every refusal is a
//! silent change of the kernel's flags (plan 5.6). Kconfig only records some of those answers, and
//! before 4.18 none of them, so this compares the command lines themselves. They come from the
//! `.<object>.cmd` file kbuild writes beside every object it built, which holds the command as
//! `savedcmd_<object> := ...`, or `cmd_<object> := ...` before 6.2.
//!
//! Each command is cut at the first `;`, since kbuild appends objtool and other steps after it,
//! and made comparable: the compiler becomes `CC`, the persona and the dependency file flags are
//! dropped along with the output, and the two output directories and source trees are written as
//! `OUT` and `SRC`. Order is kept, because it matters for `-f` pairs. Differences are counted by
//! flag, so that one flag missing from three thousand units is one line.

use crate::kconfig::Divergences;
use rk_shim::args::split_response_file;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// The compile command of every object in a build, by object path relative to the output
/// directory.
pub type Commands = BTreeMap<String, Vec<String>>;

/// The compile command in the text of a `.cmd` file, with the object it builds. Link steps and
/// anything else that does not pass `-c` are not compile commands and give nothing.
#[must_use]
pub fn parse_cmd(text: &str) -> Option<(String, Vec<String>)> {
    let line = text
        .lines()
        .find(|l| l.starts_with("savedcmd_") || l.starts_with("cmd_"))?;
    let (target, command) = line.split_once(" := ")?;
    let object = target
        .strip_prefix("savedcmd_")
        .or_else(|| target.strip_prefix("cmd_"))?;
    // make wrote `$` as `$$` and `#` as `\#` so that it could read the file back.
    let command = command.replace("$$", "$").replace("\\#", "#");
    let mut words = Vec::new();
    for word in split_response_file(&command) {
        if word == ";" {
            break;
        }
        if let Some(last) = word.strip_suffix(';') {
            if !last.is_empty() {
                words.push(last.to_string());
            }
            break;
        }
        words.push(word);
    }
    words
        .iter()
        .any(|w| w == "-c")
        .then(|| (object.to_string(), words))
}

/// Every compile command under a build's output directory. Symbolic links are not followed, so
/// kbuild's `source` link back to the tree is not walked.
pub fn load(out: &Path) -> Result<Commands, String> {
    let mut commands = Commands::new();
    let mut dirs = vec![out.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                if name != "rk-bin" {
                    dirs.push(entry.path());
                }
            } else if kind.is_file() && name.starts_with('.') && name.ends_with(".o.cmd") {
                let Ok(text) = std::fs::read_to_string(entry.path()) else {
                    continue;
                };
                if let Some((object, words)) = parse_cmd(&text) {
                    commands.insert(relative(&object, out), words);
                }
            }
        }
    }
    Ok(commands)
}

/// An object's name relative to the output directory. objtool is built by its own makefile, which
/// names its objects by their full path, so without this every one of them is in one build only.
fn relative(object: &str, out: &Path) -> String {
    Path::new(object).strip_prefix(out).map_or_else(
        |_| object.to_string(),
        |rest| rest.to_string_lossy().into_owned(),
    )
}

/// A command made comparable: `CC` for the compiler, no persona, no output or dependency file,
/// and `OUT` and `SRC` for the directories named in `dirs`, longest first.
#[must_use]
pub fn normalize(words: &[String], dirs: &[(&str, &str)]) -> Vec<String> {
    let mut dirs: Vec<&(&str, &str)> = dirs.iter().filter(|(d, _)| !d.is_empty()).collect();
    dirs.sort_by_key(|(d, _)| std::cmp::Reverse(d.len()));
    let mut out = vec!["CC".to_string()];
    let mut rest = words.iter().skip(1);
    while let Some(word) = rest.next() {
        if matches!(word.as_str(), "-o" | "-MF" | "-MT" | "-MQ") {
            rest.next();
            continue;
        }
        if word.starts_with("-fgnuc-version=")
            || word.starts_with("-fgnu-as-version=")
            || word.starts_with("-Wp,-MD,")
            || word.starts_with("-Wp,-MMD,")
            || matches!(word.as_str(), "-MD" | "-MMD")
        {
            continue;
        }
        let mut word = word.clone();
        for (dir, name) in &dirs {
            word = word.replace(dir, name);
        }
        out.push(word);
    }
    out
}

/// What `a` has that `b` does not, counting repeats.
fn missing_from(a: &[String], b: &[String]) -> Vec<String> {
    let mut left: BTreeMap<&str, usize> = BTreeMap::new();
    for w in b {
        *left.entry(w).or_default() += 1;
    }
    let mut out = Vec::new();
    for w in a {
        match left.get_mut(w.as_str()) {
            Some(n) if *n > 0 => *n -= 1,
            _ => out.push(w.clone()),
        }
    }
    out
}

/// A flag one side passed and the other did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagDifference {
    /// The flag, normalized.
    pub flag: String,
    /// True when only the reference passed it, false when only the other build did.
    pub reference_only: bool,
    /// How many units.
    pub units: usize,
    /// The first unit, by object path.
    pub example: String,
    /// Why, when `config-divergences.toml` has a `[[flags]]` rule for it.
    pub reason: Option<String>,
}

/// The comparison.
#[derive(Debug, Clone, Default)]
pub struct Comparison {
    /// Objects both builds compiled.
    pub common: usize,
    /// Flags passed by one side and not the other, most units first.
    pub flags: Vec<FlagDifference>,
    /// Objects whose commands have the same flags in a different order.
    pub reordered: Vec<String>,
    /// Objects only the reference compiled, which is usually a unit that failed in the other.
    pub reference_only: Vec<String>,
    /// Objects only the other build compiled.
    pub other_only: Vec<String>,
}

impl Comparison {
    /// Whether anything differs that no rule explains. Objects only one side built are left to
    /// `rk config-diff` and the build's own failures.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.reordered.is_empty() && self.flags.iter().all(|f| f.reason.is_some())
    }
}

/// Compare two builds' normalized commands.
#[must_use]
pub fn compare(reference: &Commands, other: &Commands, divergences: &Divergences) -> Comparison {
    let mut comparison = Comparison::default();
    let mut counted: BTreeMap<(String, bool), (usize, String)> = BTreeMap::new();
    for (object, ours) in reference {
        let Some(theirs) = other.get(object) else {
            comparison.reference_only.push(object.clone());
            continue;
        };
        comparison.common += 1;
        if ours == theirs {
            continue;
        }
        let gone = missing_from(ours, theirs);
        let added = missing_from(theirs, ours);
        if gone.is_empty() && added.is_empty() {
            comparison.reordered.push(object.clone());
        }
        for (flags, reference_only) in [(gone, true), (added, false)] {
            for flag in flags {
                let entry = counted
                    .entry((flag, reference_only))
                    .or_insert_with(|| (0, object.clone()));
                entry.0 += 1;
            }
        }
    }
    comparison.other_only = other
        .keys()
        .filter(|o| !reference.contains_key(*o))
        .cloned()
        .collect();
    comparison.flags = counted
        .into_iter()
        .map(
            |((flag, reference_only), (units, example))| FlagDifference {
                reason: divergences.flag_reason(&flag),
                flag,
                reference_only,
                units,
                example,
            },
        )
        .collect();
    comparison
        .flags
        .sort_by(|a, b| b.units.cmp(&a.units).then_with(|| a.flag.cmp(&b.flag)));
    comparison
}

/// At most this many objects are named in a list.
const NAMED: usize = 20;

fn list(s: &mut String, title: &str, objects: &[String]) {
    if objects.is_empty() {
        return;
    }
    let _ = writeln!(s, "\n{title}: {}.\n", objects.len());
    for o in objects.iter().take(NAMED) {
        let _ = writeln!(s, "- `{o}`");
    }
    if objects.len() > NAMED {
        let _ = writeln!(s, "- and {} more", objects.len() - NAMED);
    }
}

/// The comparison as markdown.
#[must_use]
pub fn report(c: &Comparison) -> String {
    let unexplained = c.flags.iter().filter(|f| f.reason.is_none()).count();
    let mut s = format!(
        "{} objects in both builds, {} flags differ ({} explained), {} objects reordered.\n",
        c.common,
        c.flags.len(),
        c.flags.len() - unexplained,
        c.reordered.len()
    );
    if !c.flags.is_empty() {
        s.push_str("\n| flag | only in | units | first unit | reason |\n|---|---|---|---|---|\n");
        for f in &c.flags {
            let _ = writeln!(
                s,
                "| `{}` | {} | {} | `{}` | {} |",
                f.flag.replace('|', "\\|"),
                if f.reference_only {
                    "reference"
                } else {
                    "other"
                },
                f.units,
                f.example,
                f.reason.as_deref().unwrap_or_default()
            );
        }
    }
    list(&mut s, "Same flags in another order", &c.reordered);
    list(&mut s, "Built only by the reference", &c.reference_only);
    list(&mut s, "Built only by the other", &c.other_only);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const GCC: &str = "savedcmd_kernel/fork.o := /g/rk-bin/rk-cc -Wp,-MMD,kernel/.fork.o.d -nostdinc -I/src/linux/include -mno-red-zone -fno-PIE -DKBUILD_MODFILE='\"kernel/fork\"' -DKBUILD_BASENAME='\"fork\"' -c -o kernel/fork.o /src/linux/kernel/fork.c   ; ./tools/objtool/objtool --hacks=jump_label kernel/fork.o\n\nsource_kernel/fork.o := /src/linux/kernel/fork.c\n";

    const RUCC: &str = "savedcmd_kernel/fork.o := /r/rk-bin/rk-cc -fgnuc-version=14.2.0 -Wp,-MMD,kernel/.fork.o.d -nostdinc -I/src/linux/include -fno-PIE -DKBUILD_MODFILE='\"kernel/fork\"' -DKBUILD_BASENAME='\"fork\"' -c -o kernel/fork.o /src/linux/kernel/fork.c\n";

    fn commands(text: &str, out: &str) -> Commands {
        let (object, words) = parse_cmd(text).unwrap();
        let mut c = Commands::new();
        c.insert(
            object,
            normalize(&words, &[(out, "OUT"), ("/src/linux", "SRC")]),
        );
        c
    }

    #[test]
    fn an_object_named_by_its_full_path_is_keyed_like_the_rest() {
        let out = Path::new("/w/out/def-gcc");
        assert_eq!(
            relative("/w/out/def-gcc/tools/objtool/elf.o", out),
            "tools/objtool/elf.o"
        );
        assert_eq!(relative("kernel/fork.o", out), "kernel/fork.o");
        assert_eq!(
            relative("/w/out/def-rucc/elf.o", out),
            "/w/out/def-rucc/elf.o"
        );
    }

    #[test]
    fn a_cmd_file_gives_the_compile_command_without_what_follows() {
        let (object, words) = parse_cmd(GCC).unwrap();
        assert_eq!(object, "kernel/fork.o");
        assert_eq!(words.last().unwrap(), "/src/linux/kernel/fork.c");
        assert!(words.contains(&"-DKBUILD_BASENAME=\"fork\"".to_string()));
        assert!(parse_cmd("savedcmd_vmlinux.a := ar cDPrST vmlinux.a init/main.o\n").is_none());
        assert!(parse_cmd("cmd_lib/x.o := gcc -c -o lib/x.o lib/x.c\n").is_some());
    }

    #[test]
    fn normalizing_drops_the_compiler_persona_output_and_dependencies() {
        let (_, words) = parse_cmd(RUCC).unwrap();
        let n = normalize(&words, &[("/r", "OUT"), ("/src/linux", "SRC")]);
        assert_eq!(
            n.join(" "),
            "CC -nostdinc -ISRC/include -fno-PIE -DKBUILD_MODFILE=\"kernel/fork\" -DKBUILD_BASENAME=\"fork\" -c SRC/kernel/fork.c"
        );
    }

    #[test]
    fn a_flag_only_the_reference_passed_is_counted_and_can_be_explained() {
        let gcc = commands(GCC, "/g");
        let rucc = commands(RUCC, "/r");
        let found = compare(&gcc, &rucc, &Divergences::default());
        assert_eq!(found.common, 1);
        assert_eq!(found.flags.len(), 1);
        assert_eq!(found.flags[0].flag, "-mno-red-zone");
        assert!(found.flags[0].reference_only);
        assert!(!found.clean());
        assert!(
            report(&found).contains("| `-mno-red-zone` | reference | 1 | `kernel/fork.o` |  |")
        );
        let rules =
            Divergences::parse("[[flags]]\nflag = \"-mno-red*\"\nreason = \"tamnd/rucc#1\"\n")
                .unwrap();
        assert!(compare(&gcc, &rucc, &rules).clean());
    }

    #[test]
    fn the_same_flags_in_another_order_are_a_difference() {
        let a = Commands::from([(
            "x.o".to_string(),
            vec!["CC".into(), "-a".into(), "-b".into()],
        )]);
        let b = Commands::from([(
            "x.o".to_string(),
            vec!["CC".into(), "-b".into(), "-a".into()],
        )]);
        let found = compare(&a, &b, &Divergences::default());
        assert!(found.flags.is_empty());
        assert_eq!(found.reordered, ["x.o"]);
        assert!(!found.clean());
    }
}
