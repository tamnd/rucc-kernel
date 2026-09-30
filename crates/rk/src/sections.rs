//! `rk sections-diff`: the sections and kernel tables of every object in two builds, compared.
//!
//! The kernel finds much of itself through tables the compiler fills: exported symbols, jump
//! labels, alternatives, exception fixups, initcalls, module parameters and more. A unit that
//! compiles and boots can still drop an entry from one of them, and the kernel then misses an
//! export or a fixup at run time and not at build time. So the rules of plan 9.4 are checked
//! for every object the two builds share:
//!
//! The set of section names is the same, once the per-function names that
//! `-ffunction-sections` and `-fdata-sections` make are folded into one, and the per-symbol
//! export sections of 5.x into theirs. The size and relocation count of every kernel table is the
//! same. The call-site lists that depend on inlining, `__mcount_loc` and the lists objtool writes,
//! are compared per function, for the functions both objects have. The strings of `.modinfo`
//! and `__ksymtab_strings` are the same.
//!
//! A build directory is read by walking its objects. `--save FILE` keeps what was found as JSON,
//! so that CI can compare two builds without carrying their objects between jobs.

use crate::objects::{self, Functions};
use object::{Object as _, ObjectSection as _};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// The tables compared whole, per object. A trailing `*` matches any rest of the name.
const TABLES: &[&str] = &[
    "__ksymtab",
    "__ksymtab_gpl",
    "__kcrctab",
    "__kcrctab_gpl",
    "__jump_table",
    ".altinstructions",
    "__ex_table",
    "__bug_table",
    ".init.setup",
    ".initcall*",
    "__param",
    ".con_initcall.init",
    "__tracepoints*",
    "_ftrace_events",
    "__syscalls_metadata",
    ".BTF_ids",
];

/// The call-site lists, which depend on inlining and are compared per function.
const SITES: &[&str] = &[
    "__mcount_loc",
    "__patchable_function_entries",
    ".static_call_sites",
    ".retpoline_sites",
    ".return_sites",
    ".call_sites",
    ".ibt_endbr_seal",
    ".orc_unwind_ip",
];

/// Prefixes after which `-ffunction-sections` and `-fdata-sections` put a symbol name, with the
/// kernel's own names that start the same way. The kernel writes its own sections with two dots,
/// `.data..percpu`, so a single dot and a name is the compiler's.
const PER_SYMBOL: &[&str] = &[
    ".text.unlikely.",
    ".text.hot.",
    ".text.startup.",
    ".text.exit.",
    ".text.",
    ".data.rel.ro.",
    ".data.rel.local.",
    ".data.rel.",
    ".data.",
    ".bss.",
    ".rodata.",
    ".tdata.",
    ".tbss.",
    ".ltext.",
];

/// Section names that look per-symbol and are not.
const NOT_PER_SYMBOL: &[&str] = &[
    ".text.unlikely",
    ".text.hot",
    ".text.startup",
    ".text.exit",
    ".data.rel",
    ".data.rel.ro",
    ".data.rel.local",
];

/// A section name with the symbol part of a per-function or per-symbol name, and the merge
/// entry size of a string or constant section, written as `*`.
#[must_use]
pub fn fold(name: &str) -> String {
    for prefix in [
        "___ksymtab_gpl+",
        "___ksymtab+",
        "___kcrctab_gpl+",
        "___kcrctab+",
    ] {
        if name.starts_with(prefix) {
            return format!("{prefix}*");
        }
    }
    if let Some(rest) = name.strip_prefix(".rodata.str")
        && rest.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return ".rodata.str*".to_string();
    }
    if let Some(rest) = name.strip_prefix(".rodata.cst")
        && rest.bytes().all(|b| b.is_ascii_digit())
    {
        return ".rodata.cst*".to_string();
    }
    if NOT_PER_SYMBOL.contains(&name) {
        return name.to_string();
    }
    for prefix in PER_SYMBOL {
        if let Some(rest) = name.strip_prefix(prefix)
            && !rest.is_empty()
            && !rest.starts_with('.')
        {
            return format!("{prefix}*");
        }
    }
    name.to_string()
}

/// The table a section is, if it is one. The per-symbol export sections count toward the
/// table they are linked into.
fn table_of(name: &str) -> Option<String> {
    for (prefix, table) in [
        ("___ksymtab_gpl+", "__ksymtab_gpl"),
        ("___ksymtab+", "__ksymtab"),
        ("___kcrctab_gpl+", "__kcrctab_gpl"),
        ("___kcrctab+", "__kcrctab"),
    ] {
        if name.starts_with(prefix) {
            return Some(table.to_string());
        }
    }
    TABLES
        .iter()
        .any(|t| match t.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => name == *t,
        })
        .then(|| name.to_string())
}

/// The size and relocation count of a table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// Bytes.
    pub size: u64,
    /// Relocations, one or more per entry.
    pub relocs: u64,
}

/// What one object holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Object {
    /// Section names, folded.
    pub sections: BTreeSet<String>,
    /// The tables compared whole.
    pub tables: BTreeMap<String, Table>,
    /// The call-site lists, entries by function.
    pub sites: BTreeMap<String, BTreeMap<String, u64>>,
    /// The strings of `.modinfo`, in order.
    pub modinfo: Vec<String>,
    /// The strings of `__ksymtab_strings`, in order.
    pub ksymtab_strings: Vec<String>,
}

/// Every object of a build by relative path.
pub type Inventory = BTreeMap<String, Object>;

fn strings(data: &[u8]) -> Vec<String> {
    data.split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Read one object.
pub fn read(data: &[u8]) -> Result<Object, String> {
    let file = object::File::parse(data).map_err(|e| e.to_string())?;
    let functions = Functions::of(&file);
    let mut out = Object::default();
    for section in file.sections() {
        let Ok(name) = section.name() else { continue };
        if name.is_empty()
            || matches!(
                section.kind(),
                object::SectionKind::Metadata | object::SectionKind::Linker
            )
            || name.starts_with(".debug")
            || name.starts_with(".rela")
        {
            continue;
        }
        out.sections.insert(fold(name));
        if SITES.contains(&name) {
            let by_function = out.sites.entry(name.to_string()).or_default();
            for (_, reloc) in section.relocations() {
                if let Some(function) = functions.target(&file, &reloc) {
                    *by_function.entry(function).or_default() += 1;
                }
            }
        } else if let Some(table) = table_of(name) {
            let entry = out.tables.entry(table).or_default();
            entry.size += section.size();
            entry.relocs += section.relocations().count() as u64;
        } else if name == ".modinfo" || name == "__ksymtab_strings" {
            let found = strings(section.data().map_err(|e| e.to_string())?);
            if name == ".modinfo" {
                out.modinfo.extend(found);
            } else {
                out.ksymtab_strings.extend(found);
            }
        }
    }
    Ok(out)
}

/// Every object under a build directory. An object that does not parse is left out, since a
/// failed build can leave a truncated one behind.
pub fn scan(out: &Path) -> Result<Inventory, String> {
    let mut inventory = Inventory::new();
    for (name, path) in objects::find(out)? {
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        if let Ok(object) = read(&data) {
            inventory.insert(name, object);
        }
    }
    Ok(inventory)
}

/// One difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// The object.
    pub object: String,
    /// What differs: `sections`, a table name, a site list name, `.modinfo` or
    /// `__ksymtab_strings`.
    pub what: String,
    /// The reference's side.
    pub reference: String,
    /// The other side.
    pub other: String,
}

/// The comparison of two builds.
#[derive(Debug, Clone, Default)]
pub struct Comparison {
    /// Objects both builds have.
    pub shared: usize,
    /// Objects only the reference has, which the other build failed to make.
    pub only_reference: Vec<String>,
    /// Objects only the other build has.
    pub only_other: Vec<String>,
    /// Every difference in a shared object.
    pub differences: Vec<Difference>,
}

impl Comparison {
    /// Whether nothing differs.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.differences.is_empty() && self.only_other.is_empty()
    }
}

fn set_difference(a: &BTreeSet<String>, b: &BTreeSet<String>) -> String {
    a.difference(b).cloned().collect::<Vec<_>>().join(" ")
}

fn list_difference(a: &[String], b: &[String]) -> String {
    let b: BTreeSet<&String> = b.iter().collect();
    a.iter()
        .filter(|s| !b.contains(s))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compare_object(name: &str, r: &Object, o: &Object, out: &mut Vec<Difference>) {
    #![allow(clippy::many_single_char_names)]
    let mut push = |what: &str, reference: String, other: String| {
        out.push(Difference {
            object: name.to_string(),
            what: what.to_string(),
            reference,
            other,
        });
    };
    if r.sections != o.sections {
        push(
            "sections",
            set_difference(&r.sections, &o.sections),
            set_difference(&o.sections, &r.sections),
        );
    }
    let tables: BTreeSet<&String> = r.tables.keys().chain(o.tables.keys()).collect();
    for table in tables {
        let a = r.tables.get(table).copied().unwrap_or_default();
        let b = o.tables.get(table).copied().unwrap_or_default();
        if a != b {
            push(
                table,
                format!("{} bytes, {} relocs", a.size, a.relocs),
                format!("{} bytes, {} relocs", b.size, b.relocs),
            );
        }
    }
    let lists: BTreeSet<&String> = r.sites.keys().chain(o.sites.keys()).collect();
    let empty = BTreeMap::new();
    for list in lists {
        let a = r.sites.get(list).unwrap_or(&empty);
        let b = o.sites.get(list).unwrap_or(&empty);
        for (function, x) in a {
            let Some(y) = b.get(function) else { continue };
            if x != y {
                push(
                    &format!("{list} in {function}"),
                    x.to_string(),
                    y.to_string(),
                );
            }
        }
    }
    if r.modinfo != o.modinfo {
        push(
            ".modinfo",
            list_difference(&r.modinfo, &o.modinfo),
            list_difference(&o.modinfo, &r.modinfo),
        );
    }
    if r.ksymtab_strings != o.ksymtab_strings {
        push(
            "__ksymtab_strings",
            list_difference(&r.ksymtab_strings, &o.ksymtab_strings),
            list_difference(&o.ksymtab_strings, &r.ksymtab_strings),
        );
    }
}

/// Compare two builds.
#[must_use]
pub fn compare(reference: &Inventory, other: &Inventory) -> Comparison {
    let mut c = Comparison::default();
    for (name, r) in reference {
        match other.get(name) {
            Some(o) => {
                c.shared += 1;
                compare_object(name, r, o, &mut c.differences);
            }
            None => c.only_reference.push(name.clone()),
        }
    }
    c.only_other = other
        .keys()
        .filter(|k| !reference.contains_key(*k))
        .cloned()
        .collect();
    c
}

/// How many rows of a long list the report shows.
const ROWS: usize = 60;

fn cell(s: &str) -> String {
    let s = if s.is_empty() { "nothing" } else { s };
    let s = s.replace('|', "\\|");
    if s.len() > 120 {
        let mut end = 117;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    } else {
        s
    }
}

/// The comparison as markdown.
#[must_use]
pub fn report(c: &Comparison) -> String {
    let mut s = String::from("### Sections and kernel tables\n\n");
    let _ = writeln!(
        s,
        "{} objects compared, {} only in the reference, {} only in the other build, {} differences.\n",
        c.shared,
        c.only_reference.len(),
        c.only_other.len(),
        c.differences.len()
    );
    let mut by_what: BTreeMap<&str, usize> = BTreeMap::new();
    for d in &c.differences {
        let what = d.what.split(" in ").next().unwrap_or(&d.what);
        *by_what.entry(what).or_default() += 1;
    }
    if !by_what.is_empty() {
        s.push_str("| what | objects or functions |\n|---|---|\n");
        for (what, n) in &by_what {
            let _ = writeln!(s, "| `{what}` | {n} |");
        }
        s.push_str("\n| object | what | reference only | other only |\n|---|---|---|---|\n");
        for d in c.differences.iter().take(ROWS) {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                d.object,
                cell(&d.what),
                cell(&d.reference),
                cell(&d.other)
            );
        }
        if c.differences.len() > ROWS {
            let _ = writeln!(s, "\nand {} more.", c.differences.len() - ROWS);
        }
        s.push('\n');
    }
    if !c.only_reference.is_empty() {
        let shown: Vec<&str> = c
            .only_reference
            .iter()
            .take(ROWS)
            .map(String::as_str)
            .collect();
        let _ = writeln!(
            s,
            "Only in the reference: {}{}\n",
            shown.join(", "),
            if c.only_reference.len() > ROWS {
                ", ..."
            } else {
                ""
            }
        );
    }
    if !c.only_other.is_empty() {
        let _ = writeln!(s, "Only in the other build: {}\n", c.only_other.join(", "));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::fixture::Builder;
    use object::elf::R_X86_64_64;

    #[test]
    fn per_function_names_fold_and_kernel_names_do_not() {
        assert_eq!(fold(".text.do_fork"), ".text.*");
        assert_eq!(fold(".text.unlikely.foo.cold"), ".text.unlikely.*");
        assert_eq!(fold(".text.unlikely"), ".text.unlikely");
        assert_eq!(fold(".data..percpu"), ".data..percpu");
        assert_eq!(fold(".text..refcount"), ".text..refcount");
        assert_eq!(fold(".rodata.str1.8"), ".rodata.str*");
        assert_eq!(fold("___ksymtab_gpl+kmalloc"), "___ksymtab_gpl+*");
        assert_eq!(fold(".init.text"), ".init.text");
        assert_eq!(table_of("___ksymtab+x").as_deref(), Some("__ksymtab"));
        assert_eq!(
            table_of(".initcall6.init").as_deref(),
            Some(".initcall6.init")
        );
        assert_eq!(table_of(".text"), None);
    }

    fn object(sites: usize, jump: usize, license: &str) -> Vec<u8> {
        let mut b = Builder::new();
        let f = b.function("f", &[0x90; 8]);
        let g = b.function("g", &[0xc3]);
        let mcount = b.section("__mcount_loc", &vec![0; 8 * (sites + 1)]);
        for i in 0..sites {
            b.reloc(mcount, 8 * i as u64, f, R_X86_64_64);
        }
        b.reloc(mcount, 8 * sites as u64, g, R_X86_64_64);
        let table = b.section("__jump_table", &vec![0; 16 * jump]);
        for i in 0..jump {
            b.reloc(table, 16 * i as u64, f, R_X86_64_64);
        }
        b.section(".modinfo", format!("license={license}\0").as_bytes());
        b.bytes()
    }

    #[test]
    fn a_missing_table_entry_and_a_changed_modinfo_are_found() {
        let reference: Inventory =
            [("a.o".to_string(), read(&object(1, 2, "GPL")).unwrap())].into();
        let same: Inventory = [("a.o".to_string(), read(&object(1, 2, "GPL")).unwrap())].into();
        assert!(compare(&reference, &same).clean());
        let r = &reference["a.o"];
        assert_eq!(r.sites["__mcount_loc"]["f"], 1);
        assert_eq!(r.sites["__mcount_loc"]["g"], 1);
        assert_eq!(r.tables["__jump_table"].relocs, 2);
        assert_eq!(r.modinfo, ["license=GPL"]);
        assert!(r.sections.contains("__jump_table"));

        let other: Inventory = [("a.o".to_string(), read(&object(3, 1, "MIT")).unwrap())].into();
        let c = compare(&reference, &other);
        let what: Vec<&str> = c.differences.iter().map(|d| d.what.as_str()).collect();
        assert_eq!(what, ["__jump_table", "__mcount_loc in f", ".modinfo"]);
        assert!(report(&c).contains("| a.o | __mcount_loc in f | 1 | 3 |"));
    }

    #[test]
    fn an_object_the_other_build_lacks_is_listed_but_not_a_difference() {
        let reference: Inventory = [
            ("a.o".to_string(), Object::default()),
            ("b.o".to_string(), Object::default()),
        ]
        .into();
        let other: Inventory = [("a.o".to_string(), Object::default())].into();
        let c = compare(&reference, &other);
        assert!(c.clean());
        assert_eq!(c.only_reference, ["b.o"]);
        assert!(report(&c).contains("Only in the reference: b.o"));
    }
}
