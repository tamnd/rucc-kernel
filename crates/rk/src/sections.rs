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
//! same, apart from the four tables whose entries are written by the code they describe:
//! `__jump_table`, `__bug_table`, `__ex_table` and `.altinstructions`. Every inlined copy of a
//! `static_branch_unlikely` or a `WARN_ON` adds its own entry, so their counts follow the
//! inliner. For those, the set of distinct sites must be the same: the static key and branch of a
//! jump label, the format, file, line and flags of a bug, the fixup type of an exception entry,
//! and the CPU feature of an alternative. A site dropped by a lost `asm goto` or `.pushsection`
//! still shows. The call-site lists that depend on inlining, `__mcount_loc` and the lists objtool
//! writes, are compared per function, for the functions both objects have. The strings of
//! `.modinfo` and `__ksymtab_strings` are the same.
//!
//! Some differences are choices each optimizer makes for itself, such as a constant pool, a cold
//! section or a check one compiler proves can never fire. `sections-divergences.toml` lists them
//! with a reason, and a difference whose every item it explains is reported apart and does not
//! fail the run.
//!
//! A build directory is read by walking its objects. `--save FILE` keeps what was found as JSON,
//! so that CI can compare two builds without carrying their objects between jobs.

use crate::objects::{self, Functions};
use object::{Object as _, ObjectSection as _, ObjectSymbol as _, RelocationTarget, SymbolKind};
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

/// The tables whose entries come with the code they describe, compared by the distinct sites
/// they hold, with the size of one entry when it is fixed. A `__bug_table` entry is as large as
/// the configuration makes it, so its size is the table's over the entries in it.
const INLINED: &[(&str, u64)] = &[
    ("__jump_table", 16),
    ("__ex_table", 12),
    (".altinstructions", 14),
    ("__bug_table", 0),
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
    /// The distinct sites of the tables in [`INLINED`].
    #[serde(default)]
    pub distinct: BTreeMap<String, BTreeSet<String>>,
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

/// The named symbols of an object by section index, sorted by address, for naming what a table
/// entry points at. The assembler may write a reference to a `static` as its section and an
/// offset, so the name has to be found again.
struct Named {
    by_section: BTreeMap<usize, Vec<(u64, u64, String)>>,
}

impl Named {
    fn of(file: &object::File<'_>) -> Self {
        let mut by_section: BTreeMap<usize, Vec<(u64, u64, String)>> = BTreeMap::new();
        for symbol in file.symbols() {
            if symbol.kind() == SymbolKind::Section || symbol.kind() == SymbolKind::File {
                continue;
            }
            let (Some(section), Ok(name)) = (symbol.section_index(), symbol.name()) else {
                continue;
            };
            if name.is_empty() || name.starts_with(".L") {
                continue;
            }
            let name = if symbol.is_local() {
                unnumbered(name)
            } else {
                name
            };
            by_section.entry(section.0).or_default().push((
                symbol.address(),
                symbol.size(),
                name.to_string(),
            ));
        }
        for list in by_section.values_mut() {
            list.sort();
        }
        Self { by_section }
    }

    /// The symbol covering an offset in a section and how far into it the offset is.
    fn at(&self, section: usize, offset: u64) -> Option<(&str, u64)> {
        let list = self.by_section.get(&section)?;
        let i = list.partition_point(|(start, _, _)| *start <= offset);
        let (start, size, name) = list.get(i.checked_sub(1)?)?;
        (offset < start + (*size).max(1)).then(|| (name.as_str(), offset - start))
    }
}

/// A local name without the number a compiler puts after it to tell apart the function-local
/// statics of one name, `___once_key.35`, which each compiler counts its own way.
fn unnumbered(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, n))
            if !stem.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) =>
        {
            stem
        }
        _ => name,
    }
}

/// What a relocation in a table entry points at, written so that it reads the same in both
/// builds: a string by its text, data by the symbol it falls in, and anything else by section
/// and offset. `None` for code, since where code lands is the compiler's business.
fn pointee(file: &object::File<'_>, reloc: &object::Relocation, named: &Named) -> Option<String> {
    let RelocationTarget::Symbol(index) = reloc.target() else {
        return Some(String::from("?"));
    };
    let symbol = file.symbol_by_index(index).ok()?;
    let name = symbol.name().unwrap_or_default();
    let Some(section) = symbol.section_index() else {
        return Some(format!("{name}{:+}", reloc.addend()));
    };
    let section = file.section_by_index(section).ok()?;
    if section.kind() == object::SectionKind::Text {
        return None;
    }
    let offset = symbol.address().wrapping_add_signed(reloc.addend());
    if section.kind() == object::SectionKind::ReadOnlyString
        || section.name().is_ok_and(|n| n.starts_with(".rodata.str"))
    {
        let data = section.data().unwrap_or_default();
        let rest = usize::try_from(offset)
            .ok()
            .and_then(|o| data.get(o..))
            .unwrap_or_default();
        let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        return Some(format!("{:?}", String::from_utf8_lossy(&rest[..end])));
    }
    if let Some((found, into)) = named.at(section.index().0, offset) {
        return Some(format!("{found}+{into}"));
    }
    Some(format!(
        "{}+{offset}",
        fold(section.name().unwrap_or_default())
    ))
}

/// The distinct sites of one of the [`INLINED`] tables. A site is everything in an entry that is
/// not the address of code: the static key and branch bit of a jump label, the strings, line and
/// flags of a bug, the type and immediate of an exception entry with the register it names left
/// out, and the CPU feature and flags of an alternative. The lengths of an alternative's
/// instructions are left out with the register, since both follow register allocation.
fn distinct(
    file: &object::File<'_>,
    section: &object::Section<'_, '_>,
    table: &str,
    size: u64,
    named: &Named,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let data = section.data().unwrap_or_default();
    let relocs: Vec<(u64, object::Relocation)> = section.relocations().collect();
    let size = if size > 0 {
        size
    } else {
        let code = relocs
            .iter()
            .filter(|(_, r)| pointee(file, r, named).is_none())
            .count() as u64;
        if code == 0 {
            return out;
        }
        section.size() / code
    };
    if size == 0 {
        return out;
    }
    let mut fields: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    let mut covered = vec![false; data.len()];
    for (offset, reloc) in &relocs {
        let width = usize::from(reloc.size() / 8).max(1);
        if let Ok(start) = usize::try_from(*offset) {
            for b in covered.iter_mut().skip(start).take(width) {
                *b = true;
            }
        }
        if let Some(to) = pointee(file, reloc, named) {
            fields
                .entry(offset / size)
                .or_default()
                .push(format!("{}={to}", offset % size));
        }
    }
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    for (i, entry) in data.chunks(size).enumerate() {
        let start = i * size;
        let mut parts = fields.remove(&(i as u64)).unwrap_or_default();
        let raw: Vec<u8> = match table {
            "__ex_table" => entry
                .get(8..12)
                .map(|d| {
                    let v = u32::from_le_bytes([d[0], d[1], d[2], d[3]]) & !0x0f00;
                    v.to_le_bytes().to_vec()
                })
                .unwrap_or_default(),
            ".altinstructions" => entry.get(8..12).unwrap_or_default().to_vec(),
            "__jump_table" => Vec::new(),
            _ => entry
                .iter()
                .enumerate()
                .filter(|(j, _)| !covered.get(start + j).copied().unwrap_or(false))
                .map(|(_, b)| *b)
                .collect(),
        };
        if !raw.is_empty() {
            parts.push(raw.iter().fold(String::new(), |mut hex, b| {
                let _ = write!(hex, "{b:02x}");
                hex
            }));
        }
        out.insert(parts.join(" "));
    }
    out
}

/// Read one object.
pub fn read(data: &[u8]) -> Result<Object, String> {
    let file = object::File::parse(data).map_err(|e| e.to_string())?;
    let functions = Functions::of(&file);
    let named = Named::of(&file);
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
            if let Some(&(_, size)) = INLINED.iter().find(|(t, _)| *t == table) {
                out.distinct
                    .entry(table.clone())
                    .or_default()
                    .extend(distinct(&file, &section, &table, size, &named));
            }
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
    /// The names or sites only the reference has, when the difference is between two sets.
    pub reference_items: Vec<String>,
    /// The names or sites only the other build has, when the difference is between two sets.
    pub other_items: Vec<String>,
    /// Why, when `sections-divergences.toml` explains every item on both sides.
    pub reason: Option<String>,
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
    /// Whether nothing differs that `sections-divergences.toml` does not explain.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.differences.iter().all(|d| d.reason.is_some()) && self.only_other.is_empty()
    }
}

/// Which build has the item a rule explains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Only {
    /// Only the reference.
    Reference,
    /// Only the other build.
    Other,
    /// Either one.
    Either,
}

/// `sections-divergences.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Divergences {
    /// Every rule.
    #[serde(default, rename = "divergence")]
    pub divergences: Vec<Divergence>,
}

/// A section name or table site that may be in one build and not the other, and why.
#[derive(Debug, Clone, Deserialize)]
pub struct Divergence {
    /// `sections` or the table, such as `__bug_table`.
    pub what: String,
    /// The object, when the rule is for one. A trailing `*` matches any object with that prefix.
    #[serde(default)]
    pub object: Option<String>,
    /// Which build has the item.
    pub only: Only,
    /// The section name or site as the report prints it. A trailing `*` matches any with that
    /// prefix, so `*` alone matches every one.
    pub item: String,
    /// Why it differs.
    pub reason: String,
}

fn matches(pattern: &str, s: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => s.starts_with(prefix),
        None => pattern == s,
    }
}

impl Divergences {
    /// Read `sections-divergences.toml`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Read the text of `sections-divergences.toml`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let divergences: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        for d in &divergences.divergences {
            if d.reason.trim().is_empty() {
                return Err(format!("{} {} has no reason", d.what, d.item));
            }
        }
        Ok(divergences)
    }

    /// The rule that explains one item of a difference, if there is one.
    fn rule(&self, d: &Difference, only: Only, item: &str) -> Option<&Divergence> {
        self.divergences.iter().find(|r| {
            r.what == d.what
                && (r.only == only || r.only == Only::Either)
                && r.object.as_deref().is_none_or(|o| matches(o, &d.object))
                && matches(&r.item, item)
        })
    }

    /// Gives every difference whose items the rules all explain its reasons.
    pub fn explain(&self, c: &mut Comparison) {
        for d in &mut c.differences {
            if d.reference_items.is_empty() && d.other_items.is_empty() {
                continue;
            }
            let sides = [
                (Only::Reference, &d.reference_items),
                (Only::Other, &d.other_items),
            ];
            let mut reasons: Vec<&str> = Vec::new();
            let mut all = true;
            for (only, items) in sides {
                for item in items {
                    match self.rule(d, only, item) {
                        Some(r) if !reasons.contains(&r.reason.as_str()) => reasons.push(&r.reason),
                        Some(_) => {}
                        None => all = false,
                    }
                }
            }
            if all {
                d.reason = Some(reasons.join(". "));
            }
        }
    }
}

fn set_difference(a: &BTreeSet<String>, b: &BTreeSet<String>) -> Vec<String> {
    a.difference(b).cloned().collect()
}

fn list_difference(a: &[String], b: &[String]) -> String {
    let b: BTreeSet<&String> = b.iter().collect();
    a.iter()
        .filter(|s| !b.contains(s))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A difference with no items, for a comparison that is not between two sets.
fn plain(object: &str, what: &str, reference: String, other: String) -> Difference {
    Difference {
        object: object.to_string(),
        what: what.to_string(),
        reference,
        other,
        reference_items: Vec::new(),
        other_items: Vec::new(),
        reason: None,
    }
}

/// A difference between two sets, with the items each side has alone.
fn between(object: &str, what: &str, a: &BTreeSet<String>, b: &BTreeSet<String>) -> Difference {
    let reference_items = set_difference(a, b);
    let other_items = set_difference(b, a);
    Difference {
        object: object.to_string(),
        what: what.to_string(),
        reference: reference_items.join(", "),
        other: other_items.join(", "),
        reference_items,
        other_items,
        reason: None,
    }
}

fn compare_object(name: &str, r: &Object, o: &Object, out: &mut Vec<Difference>) {
    #![allow(clippy::many_single_char_names)]
    if r.sections != o.sections {
        out.push(between(name, "sections", &r.sections, &o.sections));
    }
    let tables: BTreeSet<&String> = r.tables.keys().chain(o.tables.keys()).collect();
    let empty = BTreeSet::new();
    for table in tables {
        if INLINED.iter().any(|(t, _)| t == table) {
            let a = r.distinct.get(table).unwrap_or(&empty);
            let b = o.distinct.get(table).unwrap_or(&empty);
            if a != b {
                out.push(between(name, table, a, b));
            }
            continue;
        }
        let a = r.tables.get(table).copied().unwrap_or_default();
        let b = o.tables.get(table).copied().unwrap_or_default();
        if a != b {
            out.push(plain(
                name,
                table,
                format!("{} bytes, {} relocs", a.size, a.relocs),
                format!("{} bytes, {} relocs", b.size, b.relocs),
            ));
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
                out.push(plain(
                    name,
                    &format!("{list} in {function}"),
                    x.to_string(),
                    y.to_string(),
                ));
            }
        }
    }
    if r.modinfo != o.modinfo {
        out.push(plain(
            name,
            ".modinfo",
            list_difference(&r.modinfo, &o.modinfo),
            list_difference(&o.modinfo, &r.modinfo),
        ));
    }
    if r.ksymtab_strings != o.ksymtab_strings {
        out.push(plain(
            name,
            "__ksymtab_strings",
            list_difference(&r.ksymtab_strings, &o.ksymtab_strings),
            list_difference(&o.ksymtab_strings, &r.ksymtab_strings),
        ));
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

/// The comparison as markdown, unexplained differences first, with the first 60 of each kind or,
/// with `all`, every one.
#[must_use]
pub fn report(c: &Comparison, all: bool) -> String {
    let rows = if all { usize::MAX } else { ROWS };
    let (explained, unexplained): (Vec<&Difference>, Vec<&Difference>) =
        c.differences.iter().partition(|d| d.reason.is_some());
    let mut s = String::from("### Sections and kernel tables\n\n");
    let _ = writeln!(
        s,
        "{} objects compared, {} only in the reference, {} only in the other build, {} differences, {} explained and {} not.\n",
        c.shared,
        c.only_reference.len(),
        c.only_other.len(),
        c.differences.len(),
        explained.len(),
        unexplained.len()
    );
    let mut by_what: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for d in &c.differences {
        let what = d.what.split(" in ").next().unwrap_or(&d.what);
        let counts = by_what.entry(what).or_default();
        if d.reason.is_some() {
            counts.1 += 1;
        } else {
            counts.0 += 1;
        }
    }
    if !by_what.is_empty() {
        s.push_str("| what | not explained | explained |\n|---|---|---|\n");
        for (what, (not, yes)) in &by_what {
            let _ = writeln!(s, "| `{what}` | {not} | {yes} |");
        }
        s.push('\n');
    }
    let more = |s: &mut String, n: usize| {
        if n > rows {
            let _ = writeln!(s, "\nand {} more, which `--all` lists.", n - rows);
        }
        s.push('\n');
    };
    if !unexplained.is_empty() {
        s.push_str("| object | what | reference only | other only |\n|---|---|---|---|\n");
        for d in unexplained.iter().take(rows) {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                d.object,
                cell(&d.what),
                cell(&d.reference),
                cell(&d.other)
            );
        }
        more(&mut s, unexplained.len());
    }
    if !explained.is_empty() {
        s.push_str("Explained:\n\n| object | what | reference only | other only | reason |\n|---|---|---|---|---|\n");
        for d in explained.iter().take(rows) {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} |",
                d.object,
                cell(&d.what),
                cell(&d.reference),
                cell(&d.other),
                d.reason.as_deref().unwrap_or_default().replace('|', "\\|")
            );
        }
        more(&mut s, explained.len());
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
        assert_eq!(unnumbered("___once_key.35"), "___once_key");
        assert_eq!(unnumbered("x.constprop.0"), "x.constprop");
        assert_eq!(unnumbered("version.3a"), "version.3a");
        assert_eq!(unnumbered(".7"), ".7");
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
        let keys: Vec<_> = (0..jump)
            .map(|i| b.data(&format!("key{i}"), &[0; 16]))
            .collect();
        for (i, key) in keys.into_iter().enumerate() {
            b.reloc(table, 16 * i as u64, f, R_X86_64_64);
            b.reloc(table, 16 * i as u64 + 4, g, R_X86_64_64);
            b.reloc(table, 16 * i as u64 + 8, key, R_X86_64_64);
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
        assert_eq!(r.tables["__jump_table"].relocs, 6);
        assert_eq!(
            r.distinct["__jump_table"],
            ["8=key0+0".to_string(), "8=key1+0".to_string()].into()
        );
        assert_eq!(r.modinfo, ["license=GPL"]);
        assert!(r.sections.contains("__jump_table"));

        let other: Inventory = [("a.o".to_string(), read(&object(3, 1, "MIT")).unwrap())].into();
        let c = compare(&reference, &other);
        let what: Vec<&str> = c.differences.iter().map(|d| d.what.as_str()).collect();
        assert_eq!(what, ["__jump_table", "__mcount_loc in f", ".modinfo"]);
        assert!(report(&c, false).contains("| a.o | __mcount_loc in f | 1 | 3 |"));
        assert!(report(&c, false).contains("| a.o | __jump_table | 8=key1+0 | nothing |"));
    }

    /// Two copies of one site, as when a function with a static branch is inlined twice.
    fn copies(n: usize) -> Vec<u8> {
        let mut b = Builder::new();
        let f = b.function("f", &[0x90; 8]);
        let key = b.data("key", &[0; 16]);
        let table = b.section("__jump_table", &vec![0; 16 * n]);
        for i in 0..n as u64 {
            b.reloc(table, 16 * i, f, R_X86_64_64);
            b.reloc(table, 16 * i + 4, f, R_X86_64_64);
            b.reloc(table, 16 * i + 8, key, R_X86_64_64);
        }
        let bug = b.section("__bug_table", &vec![0; 12 * n]);
        for i in 0..n as u64 {
            b.reloc(bug, 12 * i, f, R_X86_64_64);
        }
        b.bytes()
    }

    #[test]
    fn more_copies_of_the_same_site_are_not_a_difference() {
        let reference: Inventory = [("a.o".to_string(), read(&copies(1)).unwrap())].into();
        let other: Inventory = [("a.o".to_string(), read(&copies(3)).unwrap())].into();
        assert_eq!(other["a.o"].tables["__jump_table"].relocs, 9);
        assert_eq!(other["a.o"].distinct["__bug_table"].len(), 1);
        assert!(compare(&reference, &other).clean());
    }

    #[test]
    fn a_difference_is_explained_only_when_every_item_is() {
        let names =
            |n: &[&str]| -> BTreeSet<String> { n.iter().map(ToString::to_string).collect() };
        let mut c = Comparison {
            shared: 2,
            differences: vec![
                between("a.o", "sections", &names(&[".rodata.cst16"]), &names(&[])),
                between(
                    "b.o",
                    "sections",
                    &names(&[".rodata.cst16"]),
                    &names(&[".data.rucc"]),
                ),
                between("c.o", "__bug_table", &names(&[]), &names(&["8=\"c.c\" 01"])),
                between("c.o", "__jump_table", &names(&["8=key"]), &names(&[])),
            ],
            ..Comparison::default()
        };
        let rules = Divergences::parse(
            r#"
[[divergence]]
what = "sections"
only = "reference"
item = ".rodata.cst*"
reason = "a pool"

[[divergence]]
what = "__bug_table"
only = "other"
item = "*"
reason = "a kept check"

[[divergence]]
what = "__jump_table"
object = "d.o"
only = "reference"
item = "8=key"
reason = "another object"
"#,
        )
        .unwrap();
        rules.explain(&mut c);
        let reasons: Vec<Option<&str>> =
            c.differences.iter().map(|d| d.reason.as_deref()).collect();
        assert_eq!(reasons, [Some("a pool"), None, Some("a kept check"), None]);
        assert!(!c.clean());
        let text = report(&c, false);
        assert!(
            text.contains("4 differences, 2 explained and 2 not."),
            "{text}"
        );
        assert!(text.contains("| `sections` | 1 | 1 |"), "{text}");
        c.differences.retain(|d| d.reason.is_some());
        assert!(c.clean());
    }

    #[test]
    fn the_divergences_file_parses_and_every_rule_has_a_reason() {
        let rules = Divergences::parse(include_str!("../../../sections-divergences.toml")).unwrap();
        assert!(!rules.divergences.is_empty());
        assert!(Divergences::parse("[[divergence]]\nwhat = \"sections\"\nonly = \"either\"\nitem = \".x\"\nreason = \" \"\n").is_err());
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
        assert!(report(&c, false).contains("Only in the reference: b.o"));
    }
}
