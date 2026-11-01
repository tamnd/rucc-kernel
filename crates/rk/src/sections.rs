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
//! writes, are compared per function, for the functions both objects have. A module whose count
//! for a function differs only because its members inlined it differently is explained by them.
//! The strings of `.modinfo` and `__ksymtab_strings` are the same. The `depends=` of `.modinfo`
//! is compared as a set, because modpost names the modules in the order it meets their symbols
//! in the module's symbol table, and that order is the compiler's. A `.ko` is the link of its
//! object, its `.mod.o` and `.module-common.o`, so it has them as members and is explained by
//! them the same way.
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
/// the configuration makes it, so its size is the table's over the entries in it. The size given
/// for `.altinstructions` is the one from 6.3 on, and an older table is read at its own.
const INLINED: &[(&str, u64)] = &[
    ("__jump_table", 16),
    ("__ex_table", 12),
    (".altinstructions", 14),
    ("__bug_table", 0),
];

/// The call-site lists the compiler writes, one entry for each function it instruments, which
/// depend on inlining and are compared per function.
///
/// The lists objtool writes, `.static_call_sites`, `.retpoline_sites`, `.return_sites`,
/// `.call_sites`, `.ibt_endbr_seal` and `.orc_unwind_ip`, are not. objtool builds them from the
/// code it reads, an ORC entry for every change to the stack and a return site for every `ret`,
/// so their counts are each compiler's own instructions, and whether they are right is what
/// `rk objtool-report` checks. With IBT, as in 6.12 and 7.2 `defconfig`, objtool writes them only
/// into `vmlinux.o`, and 6.1 writes them into every object.
const SITES: &[&str] = &["__mcount_loc", "__patchable_function_entries"];

/// The sections objtool writes into an object, which are left out of the sections an object has
/// for the same reason they are left out of [`SITES`]: whether one is there follows from the code,
/// a `.retpoline_sites` from an indirect call and a `.return_sites` from a `ret`.
const OBJTOOL: &[&str] = &[
    ".static_call_sites",
    ".retpoline_sites",
    ".return_sites",
    ".call_sites",
    ".ibt_endbr_seal",
    ".orc_unwind",
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
    /// The objects a module was linked from with `ld -r`, as the `.mod` file Kbuild writes next
    /// to it names them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
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
            if name.is_empty() || name.starts_with(".L") || mapping(name) {
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

/// Whether a name is one of the mapping symbols the Arm ELF ABI has the assembler put where code
/// and data start, `$x` and `$d` on arm64 and `$a` and `$t` on 32-bit Arm, with or without a `.`
/// and more after it. GNU as writes them and an assembler need not, so they name nothing.
fn mapping(name: &str) -> bool {
    matches!(
        name.as_bytes(),
        [b'$', b'x' | b'd' | b'a' | b't'] | [b'$', b'x' | b'd' | b'a' | b't', b'.', ..]
    )
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

/// The text a pointer in a bug entry points at, read up to its NUL. Every pointer in a bug entry
/// that is not code is a string, the file and in the x86 entries of 7.2 the format, but not every
/// string is in a string section. The nVHE hypervisor of arm64 links its `.rodata.str*` into
/// `.hyp.rodata` with the rest of its read only data, and the labels its entries point through
/// are numbered in the order the assembler met them, so without the text the same WARN reads as
/// `.hyp.rodata+1027` in one build and `.hyp.rodata+1008` in the other.
fn text(file: &object::File<'_>, reloc: &object::Relocation) -> Option<String> {
    let RelocationTarget::Symbol(index) = reloc.target() else {
        return None;
    };
    let symbol = file.symbol_by_index(index).ok()?;
    let section = file.section_by_index(symbol.section_index()?).ok()?;
    if section.kind() == object::SectionKind::Text {
        return None;
    }
    let data = section.data().ok()?;
    let offset = usize::try_from(symbol.address().wrapping_add_signed(reloc.addend())).ok()?;
    let rest = data.get(offset..)?;
    let end = rest.iter().position(|b| *b == 0)?;
    let text = std::str::from_utf8(&rest[..end]).ok()?;
    Some(format!("{text:?}"))
}

/// The distinct sites of one of the [`INLINED`] tables. A site is everything in an entry that is
/// not the address of code: the static key and branch bit of a jump label, the strings, line and
/// flags of a bug, the type and immediate of an exception entry with the registers it names left
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
    // An alternative is two offsets with a relocation each, the feature and two lengths. The
    // feature is a u16 before 6.3 and a u32 with flags from then on, so the entry is 12 or 14
    // bytes and the table says which.
    let size = if table == ".altinstructions" && relocs.len() >= 2 {
        let entries = (relocs.len() / 2) as u64;
        if section.size() % entries == 0 {
            section.size() / entries
        } else {
            size
        }
    } else if size > 0 {
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
        let to = match table {
            "__bug_table" => text(file, reloc).or_else(|| pointee(file, reloc, named)),
            _ => pointee(file, reloc, named),
        };
        if let Some(to) = to {
            fields
                .entry(offset / size)
                .or_default()
                .push(format!("{}={to}", offset % size));
        }
    }
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    // An x86 entry's data is an `int` with the register in bits 8 to 11 and flags and an
    // immediate above them. An AArch64 entry has a `short` type and a `short` of data, which is
    // two registers of five bits each for every type that has any. The one type whose data is not
    // registers, the `UACCESS_CPY` of 7.x with its write flag, is only written by the copy
    // routines in assembly, so leaving its flag out with the registers loses nothing a compiler
    // chose.
    let registers: u32 = if file.architecture() == object::Architecture::Aarch64 {
        0x03ff_0000
    } else {
        0x0f00
    };
    for (i, entry) in data.chunks(size).enumerate() {
        let start = i * size;
        let mut parts = fields.remove(&(i as u64)).unwrap_or_default();
        let raw: Vec<u8> = match table {
            "__ex_table" => entry
                .get(8..12)
                .map(|d| {
                    let v = u32::from_le_bytes([d[0], d[1], d[2], d[3]]) & !registers;
                    v.to_le_bytes().to_vec()
                })
                .unwrap_or_default(),
            ".altinstructions" => entry
                .get(8..entry.len().saturating_sub(2))
                .unwrap_or_default()
                .to_vec(),
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
            || OBJTOOL.contains(&name)
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
        if let Ok(mut object) = read(&data) {
            object.members = members(&path, &name);
            inventory.insert(name, object);
        }
    }
    Ok(inventory)
}

/// The members of a module, from the `.mod` file next to it, which lists each one on a line of its
/// own as a path from the top of the build. A module built from one file lists only itself, and
/// any other object has no `.mod` file at all.
fn members(path: &Path, name: &str) -> Vec<String> {
    let object = |m: &str| Path::new(m).extension().is_some_and(|e| e == "o");
    if !object(name) {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(path.with_extension("mod")) else {
        return Vec::new();
    };
    text.split_whitespace()
        .map(tidy)
        .filter(|m| object(m) && m != name)
        .collect()
}

/// A path from the top of the build with its `.` and `..` resolved, as the objects are named.
/// Kbuild writes a member from another directory the way the Makefile names it, so amdgpu's `.mod`
/// has `drivers/gpu/drm/amd/amdgpu/../amdkfd/kfd_module.o`.
fn tidy(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            _ => parts.push(part),
        }
    }
    parts.join("/")
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
    /// The members of each module linked from more than one object.
    pub members: BTreeMap<String, Vec<String>>,
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

/// A bug entry's site without the condition text of its WARN.
///
/// With `DEBUG_BUGVERBOSE_DETAILED` a `WARN_ON(cond)` puts `#cond` where the format of a `WARN`
/// goes, so allmodconfig prints `4="!p" 8="mm/x.c" 1c000309` for the site defconfig prints as
/// `4="" 8="mm/x.c" 1c000309`. The file, line and flags still name it.
fn uncondition(item: &str) -> Option<String> {
    let rest = item.strip_prefix("4=\"")?;
    let at = rest.find("\" 8=\"")?;
    (at > 0).then(|| format!("4=\"\"{}", &rest[at + 1..]))
}

/// Whether a rule for `object` follows `item` into the module `object` is linked into.
///
/// A bug entry names the file of its WARN or BUG. One that names `fs/btrfs/volumes.c` was compiled
/// into `fs/btrfs/volumes.o`, so in `fs/btrfs/btrfs.o` it is the same site, linked in with `ld -r`,
/// and the rule written for the member holds for the module. An entry that names a header could
/// have come from any member, so it gets no such help and needs a rule of its own.
fn linked(object: &str, item: &str) -> bool {
    let Some(stem) = object.strip_suffix(".o") else {
        return false;
    };
    let Some(file) = item.split('"').skip(1).step_by(2).find(|s| !s.is_empty()) else {
        return false;
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in file.split('/') {
        match part {
            ".." => {
                parts.pop();
            }
            "." => {}
            _ => parts.push(part),
        }
    }
    parts.join("/").strip_suffix(".c") == Some(stem)
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
    ///
    /// A bug entry with the condition text of its WARN is the site a rule names without it.
    fn rule(&self, d: &Difference, only: Only, item: &str) -> Option<&Divergence> {
        let bare = uncondition(item);
        self.divergences.iter().find(|r| {
            r.what == d.what
                && (r.only == only || r.only == Only::Either)
                && r.object
                    .as_deref()
                    .is_none_or(|o| matches(o, &d.object) || linked(o, item))
                && (matches(&r.item, item) || bare.as_deref().is_some_and(|b| matches(&r.item, b)))
        })
    }

    /// Gives every difference whose items the rules all explain its reasons.
    pub fn explain(&self, c: &mut Comparison) {
        let mut at: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
        for (i, d) in c.differences.iter().enumerate() {
            at.entry((d.object.as_str(), d.what.as_str()))
                .or_default()
                .push(i);
        }
        let mut found = Vec::with_capacity(c.differences.len());
        for d in &c.differences {
            if d.reference_items.is_empty() && d.other_items.is_empty() {
                found.push(None);
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
                    all &= self.item(c, &at, d, only, item, &mut reasons, MEMBERS);
                }
            }
            found.push(all.then(|| reasons.join(". ")));
        }
        for (d, reason) in c.differences.iter_mut().zip(found) {
            if reason.is_some() {
                d.reason = reason;
            }
        }
    }

    /// Whether one item of a difference is explained, adding the reasons that explain it.
    ///
    /// Where no rule does and the object is a module, `ld -r` gave it the union of what its
    /// members have. An item only one side of the module has is then only on that side in every
    /// member that has it, and it is explained when each of those is, by the same reasons. So a
    /// WARN in a header, which could have come from any member, is explained in `fs/f2fs/f2fs.o`
    /// by the rule written for `fs/f2fs/inline.o`, the one member it is in.
    #[allow(clippy::too_many_arguments)]
    fn item<'a>(
        &'a self,
        c: &Comparison,
        at: &BTreeMap<(&str, &str), Vec<usize>>,
        d: &Difference,
        only: Only,
        item: &str,
        reasons: &mut Vec<&'a str>,
        depth: u32,
    ) -> bool {
        if let Some(r) = self.rule(d, only, item) {
            if !reasons.contains(&r.reason.as_str()) {
                reasons.push(&r.reason);
            }
            return true;
        }
        let Some(depth) = depth.checked_sub(1) else {
            return false;
        };
        let Some(members) = c.members.get(&d.object) else {
            return false;
        };
        let mut any = false;
        for member in members {
            let Some(found) = at.get(&(member.as_str(), d.what.as_str())) else {
                continue;
            };
            for &i in found {
                let m = &c.differences[i];
                let items = match only {
                    Only::Reference => &m.reference_items,
                    _ => &m.other_items,
                };
                if !items.iter().any(|x| x == item) {
                    continue;
                }
                if !self.item(c, at, m, only, item, reasons, depth) {
                    return false;
                }
                any = true;
            }
        }
        any
    }
}

/// How deep [`Divergences::item`] follows a module into its members, which are modules
/// themselves only where `ld -r` was run on the output of another.
const MEMBERS: u32 = 4;

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

/// The strings of `.modinfo` with the modules of `depends=` sorted.
fn modinfo(strings: &[String]) -> Vec<String> {
    strings
        .iter()
        .map(|s| match s.strip_prefix("depends=") {
            Some(list) => {
                let mut names: Vec<&str> = list.split(',').filter(|n| !n.is_empty()).collect();
                names.sort_unstable();
                format!("depends={}", names.join(","))
            }
            None => s.clone(),
        })
        .collect()
}

/// The object every `.ko` is linked with since 6.12, which kbuild builds at the top.
const MODULE_COMMON: &str = ".module-common.o";

/// The members of a `.ko`: the object of the module, its `.mod.o` and `.module-common.o`, of
/// those the builds have.
fn ko_members(name: &str, reference: &Inventory, other: &Inventory) -> Vec<String> {
    let Some(stem) = name.strip_suffix(".ko") else {
        return Vec::new();
    };
    [
        format!("{stem}.o"),
        format!("{stem}.mod.o"),
        MODULE_COMMON.to_string(),
    ]
    .into_iter()
    .filter(|m| reference.contains_key(m) || other.contains_key(m))
    .collect()
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
    let (a, b) = (modinfo(&r.modinfo), modinfo(&o.modinfo));
    if a != b {
        let (x, y): (BTreeSet<String>, BTreeSet<String>) =
            (a.iter().cloned().collect(), b.iter().cloned().collect());
        if x == y {
            out.push(plain(
                name,
                ".modinfo",
                "the same strings in another order".to_string(),
                a.join(" "),
            ));
        } else {
            out.push(between(name, ".modinfo", &x, &y));
        }
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

/// The object the kernel generates from its export list, which `rk symvers-diff` compares symbol
/// by symbol. With `CONFIG_TRIM_UNUSED_KSYMS` it holds only the exports some module uses, so its
/// tables follow the calls each compiler made in the modules.
const EXPORTS: &str = ".vmlinux.export.o";

/// Compare two builds.
#[must_use]
pub fn compare(reference: &Inventory, other: &Inventory) -> Comparison {
    let mut c = Comparison::default();
    for (name, object) in reference.iter().chain(other) {
        if !object.members.is_empty() && !c.members.contains_key(name) {
            let members = object.members.iter().map(|m| tidy(m)).collect();
            c.members.insert(name.clone(), members);
        }
        let ko = ko_members(name, reference, other);
        if !ko.is_empty() {
            c.members.insert(name.clone(), ko);
        }
    }
    for (name, r) in reference {
        if name == EXPORTS {
            continue;
        }
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
        .filter(|k| !reference.contains_key(*k) && *k != EXPORTS)
        .cloned()
        .collect();
    for d in &mut c.differences {
        let Some((list, function)) = d.what.split_once(" in ") else {
            continue;
        };
        let builds = (reference, other, &c.members);
        if SITES.contains(&list) && copies(builds, &d.object, list, function, MEMBERS) {
            d.reason = Some(COPIES.to_string());
        }
    }
    c
}

/// Why a module's count of a function's sites can differ when its members' counts do not.
const COPIES: &str = "the module has one site for each member that kept the function out of \
line, and the members differ only in which of them inlined every call";

/// Whether a function's sites in a module differ only because the members that kept it out of
/// line differ.
///
/// `ld -r` gives a module the sites of all its members, so a static inline function from a header
/// has one site for each member that kept a copy of it. A member where one compiler inlined every
/// call has no copy in that build, which is a choice of the inliner and is not compared in the
/// member itself. Each member must have as many sites in both builds where both have the function,
/// or differ only because its own members do, and the members' sites must add up to the module's.
fn copies(
    builds: (&Inventory, &Inventory, &BTreeMap<String, Vec<String>>),
    module: &str,
    list: &str,
    function: &str,
    depth: u32,
) -> bool {
    let (reference, other, members) = builds;
    let count = |inventory: &Inventory, object: &str| {
        inventory
            .get(object)
            .and_then(|o| o.sites.get(list))
            .and_then(|s| s.get(function))
            .copied()
            .unwrap_or_default()
    };
    let Some(depth) = depth.checked_sub(1) else {
        return false;
    };
    let Some(members) = members.get(module) else {
        return false;
    };
    let (mut x, mut y) = (0, 0);
    for member in members {
        let (a, b) = (count(reference, member), count(other, member));
        if a != b && a != 0 && b != 0 && !copies(builds, member, list, function, depth) {
            return false;
        }
        x += a;
        y += b;
    }
    x == count(reference, module) && y == count(other, module)
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

    #[test]
    fn a_list_objtool_writes_is_not_a_section_the_object_has() {
        let with = |extra: &[&str]| {
            let mut b = Builder::new();
            b.function("f", &[0xc3]);
            for name in extra {
                b.section(name, &[0; 4]);
            }
            read(&b.bytes()).unwrap()
        };
        let reference: Inventory = [("a.o".to_string(), with(&[]))].into();
        let other: Inventory = [(
            "a.o".to_string(),
            with(&[".retpoline_sites", ".return_sites"]),
        )]
        .into();
        assert!(compare(&reference, &other).clean());
        let other: Inventory = [("a.o".to_string(), with(&[".init.rodata"]))].into();
        assert!(!compare(&reference, &other).clean());
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

    /// An alternatives table of two entries for the features given, laid out as 6.1 writes it
    /// with a u16 feature or as 6.3 and later write it with a u32.
    fn alternatives(features: [u32; 2], wide: bool) -> Vec<u8> {
        let mut b = Builder::new();
        let f = b.function("f", &[0x90; 8]);
        let size = if wide { 14 } else { 12 };
        let mut data = Vec::new();
        for feature in features {
            data.extend_from_slice(&[0; 8]);
            if wide {
                data.extend_from_slice(&feature.to_le_bytes());
            } else {
                data.extend_from_slice(&feature.to_le_bytes()[..2]);
            }
            data.extend_from_slice(&[5, 5]);
        }
        let table = b.section(".altinstructions", &data);
        for i in 0..2 {
            b.reloc(table, size * i, f, R_X86_64_64);
            b.reloc(table, size * i + 4, f, R_X86_64_64);
        }
        b.bytes()
    }

    #[test]
    fn an_alternative_is_its_feature_at_either_entry_size() {
        for wide in [false, true] {
            let read = |features| read(&alternatives(features, wide)).unwrap();
            let sites = &read([0x0115, 0x0204]).distinct[".altinstructions"];
            let expected: BTreeSet<String> = if wide {
                ["15010000".to_string(), "04020000".to_string()].into()
            } else {
                ["1501".to_string(), "0402".to_string()].into()
            };
            assert_eq!(*sites, expected, "wide {wide}");
            let reference: Inventory = [("a.o".to_string(), read([0x0115, 0x0204]))].into();
            let other: Inventory = [("a.o".to_string(), read([0x0204, 0x0115]))].into();
            assert!(compare(&reference, &other).clean(), "wide {wide}");
        }
    }

    /// An arm64 exception table of one entry with the type and data given, and an
    /// alternative whose replacement is found through a mapping symbol when `mapped`.
    fn arm64(kind: u16, data: u16, mapped: bool) -> Vec<u8> {
        use object::elf::R_AARCH64_PREL32;
        use object::write::{Symbol, SymbolSection};
        let mut b = Builder::aarch64();
        let f = b.function("f", &[0; 8]);
        let mut entry = vec![0; 8];
        entry.extend_from_slice(&kind.to_le_bytes());
        entry.extend_from_slice(&data.to_le_bytes());
        let table = b.section("__ex_table", &entry);
        b.reloc(table, 0, f, R_AARCH64_PREL32);
        b.reloc(table, 4, f, R_AARCH64_PREL32);
        let code = b.section(".idmap.text", &[0; 8]);
        let at = if mapped {
            b.obj.add_symbol(Symbol {
                name: b"$x".to_vec(),
                value: 4,
                size: 0,
                kind: object::SymbolKind::Label,
                scope: object::SymbolScope::Compilation,
                weak: false,
                section: SymbolSection::Section(code),
                flags: object::SymbolFlags::None,
            })
        } else {
            b.obj.section_symbol(code)
        };
        let alt = b.section(".altinstructions", &[0, 0, 0, 0, 0, 0, 0, 0, 0x3e, 0, 4, 4]);
        b.reloc(alt, 0, f, R_AARCH64_PREL32);
        b.obj
            .add_relocation(
                alt,
                object::write::Relocation {
                    offset: 4,
                    symbol: at,
                    addend: if mapped { 0 } else { 4 },
                    flags: object::RelocationFlags::Elf {
                        r_type: R_AARCH64_PREL32,
                    },
                },
            )
            .unwrap();
        b.bytes()
    }

    #[test]
    fn an_arm64_table_entry_leaves_out_its_registers_and_mapping_symbols() {
        // x20 against x22 for the error, both with xzr for the zero.
        let reference = read(&arm64(2, 0x03f4, true)).unwrap();
        let other = read(&arm64(2, 0x03f6, false)).unwrap();
        assert_eq!(
            reference.distinct["__ex_table"],
            ["02000000".to_string()].into()
        );
        assert_eq!(
            reference.distinct[".altinstructions"],
            other.distinct[".altinstructions"]
        );
        let reference: Inventory = [("a.o".to_string(), reference)].into();
        let other: Inventory = [("a.o".to_string(), other)].into();
        assert!(compare(&reference, &other).clean());
        let kaccess: Inventory =
            [("a.o".to_string(), read(&arm64(3, 0x03f4, true)).unwrap())].into();
        assert!(!compare(&reference, &kaccess).clean());
        assert!(mapping("$x") && mapping("$d.12") && !mapping("$xyz") && !mapping("x"));
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

    /// A module linked from its members has every site they have. A rule for a member explains a
    /// site that names the member's own file in the module too, and one that names a header only
    /// in the member it was written for.
    #[test]
    fn a_rule_for_a_member_follows_its_own_sites_into_the_module() {
        let site = r#"4="drivers/gpu/drm/amd/amdgpu/../amdkfd/kfd_events.c" 53020909"#;
        let header = r#"4="include/linux/highmem.h" 4b020909"#;
        let names =
            |n: &[&str]| -> BTreeSet<String> { n.iter().map(ToString::to_string).collect() };
        let mut c = Comparison {
            differences: vec![
                between(
                    "drivers/gpu/drm/amd/amdkfd/kfd_events.o",
                    "__bug_table",
                    &names(&[site]),
                    &names(&[]),
                ),
                between(
                    "drivers/gpu/drm/amd/amdgpu/amdgpu.o",
                    "__bug_table",
                    &names(&[site]),
                    &names(&[]),
                ),
                between(
                    "drivers/gpu/drm/amd/amdgpu/amdgpu.o",
                    "__bug_table",
                    &names(&[header]),
                    &names(&[]),
                ),
                between(
                    "drivers/gpu/drm/amd/amdkfd/kfd_svm.o",
                    "__bug_table",
                    &names(&[site.replace("kfd_events", "kfd_svm").as_str()]),
                    &names(&[]),
                ),
            ],
            ..Comparison::default()
        };
        let rules = Divergences::parse(&format!(
            r#"
[[divergence]]
what = "__bug_table"
object = "drivers/gpu/drm/amd/amdkfd/kfd_events.o"
only = "reference"
item = '{site}'
reason = "checked"

[[divergence]]
what = "__bug_table"
object = "drivers/gpu/drm/amd/amdkfd/kfd_events.o"
only = "reference"
item = '{header}'
reason = "checked in the member"
"#
        ))
        .unwrap();
        rules.explain(&mut c);
        let reasons: Vec<Option<&str>> =
            c.differences.iter().map(|d| d.reason.as_deref()).collect();
        assert_eq!(reasons, [Some("checked"), Some("checked"), None, None]);
    }

    #[test]
    fn a_member_from_another_directory_is_named_as_the_objects_are() {
        assert_eq!(
            tidy("drivers/gpu/drm/amd/amdgpu/../amdkfd/kfd_module.o"),
            "drivers/gpu/drm/amd/amdkfd/kfd_module.o"
        );
        assert_eq!(tidy("fs/f2fs/./inline.o"), "fs/f2fs/inline.o");
    }

    /// A module counts one site for each member that kept a function out of line. A member that
    /// inlined every call in one build explains the module's count, a member whose own count
    /// differs does not, and a module whose count is not its members' added up is not explained.
    #[test]
    fn a_module_count_that_follows_its_members_is_explained() {
        let with = |sites: &[(&str, u64)], members: &[&str]| Object {
            sites: [(
                "__mcount_loc".to_string(),
                sites.iter().map(|(f, n)| (f.to_string(), *n)).collect(),
            )]
            .into(),
            members: members.iter().map(ToString::to_string).collect(),
            ..Object::default()
        };
        let build = |module: u64, a: u64, b: u64| -> Inventory {
            let at = |n: u64| if n == 0 { vec![] } else { vec![("f", n)] };
            [
                ("m.o".to_string(), with(&at(module), &["a.o", "b.o"])),
                ("a.o".to_string(), with(&at(a), &[])),
                ("b.o".to_string(), with(&at(b), &[])),
            ]
            .into()
        };
        let reasons = |reference: &Inventory, other: &Inventory| -> Vec<(String, bool)> {
            compare(reference, other)
                .differences
                .iter()
                .map(|d| (d.object.clone(), d.reason.is_some()))
                .collect()
        };
        let reference = build(2, 1, 1);
        assert_eq!(
            reasons(&reference, &build(1, 1, 0)),
            [("m.o".to_string(), true)]
        );
        assert_eq!(
            reasons(&reference, &build(3, 1, 2)),
            [("b.o".to_string(), false), ("m.o".to_string(), false)]
        );
        assert_eq!(
            reasons(&reference, &build(3, 1, 0)),
            [("m.o".to_string(), false)]
        );
    }

    /// A module item no rule names is explained by its members, when every member that has it on
    /// that side has it explained. One member without a reason, or no member with the item at all,
    /// leaves it as it was.
    #[test]
    fn a_module_item_is_explained_by_the_members_it_came_from() {
        let header = r#"4="include/linux/highmem.h" 4b020909"#;
        let other = r#"4="include/linux/skbuff.h" 300b0309"#;
        let names =
            |n: &[&str]| -> BTreeSet<String> { n.iter().map(ToString::to_string).collect() };
        let module =
            |items: &[&str]| between("fs/f2fs/f2fs.o", "__bug_table", &names(items), &names(&[]));
        let mut c = Comparison {
            differences: vec![
                module(&[header]),
                between(
                    "fs/f2fs/inline.o",
                    "__bug_table",
                    &names(&[header]),
                    &names(&[]),
                ),
                between(
                    "fs/f2fs/data.o",
                    "sections",
                    &names(&[".text.unlikely"]),
                    &names(&[]),
                ),
                module(&[header, other]),
                between("net/x/x.o", "__bug_table", &names(&[header]), &names(&[])),
                between("net/x/a.o", "__bug_table", &names(&[header]), &names(&[])),
                between("net/x/b.o", "__bug_table", &names(&[header]), &names(&[])),
            ],
            members: [
                (
                    "fs/f2fs/f2fs.o",
                    &["fs/f2fs/inline.o", "fs/f2fs/data.o"][..],
                ),
                ("net/x/x.o", &["net/x/a.o", "net/x/b.o"][..]),
            ]
            .into_iter()
            .map(|(m, of)| (m.to_string(), of.iter().map(ToString::to_string).collect()))
            .collect(),
            ..Comparison::default()
        };
        let rules = Divergences::parse(&format!(
            r#"
[[divergence]]
what = "__bug_table"
object = "fs/f2fs/inline.o"
only = "reference"
item = '{header}'
reason = "checked in inline.c"

[[divergence]]
what = "__bug_table"
object = "net/x/a.o"
only = "reference"
item = '{header}'
reason = "checked in a.c"
"#
        ))
        .unwrap();
        rules.explain(&mut c);
        let reasons: Vec<Option<&str>> =
            c.differences.iter().map(|d| d.reason.as_deref()).collect();
        assert_eq!(
            reasons,
            [
                Some("checked in inline.c"),
                Some("checked in inline.c"),
                None,
                None,
                None,
                Some("checked in a.c"),
                None
            ]
        );
    }

    /// A `.ko` has its object, its `.mod.o` and `.module-common.o` as members, so a rule for the
    /// `.modinfo` of the `.mod.o` explains the `.ko`. The modules of `depends=` are a set, and
    /// another order is no difference.
    #[test]
    fn a_ko_is_explained_by_its_mod_o_and_depends_is_a_set() {
        let with = |modinfo: &str| {
            let mut b = Builder::new();
            b.function("f", &[0xc3]);
            b.section(".modinfo", modinfo.as_bytes());
            read(&b.bytes()).unwrap()
        };
        let build = |depends: &str| -> Inventory {
            let modinfo = format!("license=GPL\0depends={depends}\0");
            [
                ("a.o".to_string(), with("license=GPL\0")),
                ("a.mod.o".to_string(), with(&modinfo)),
                ("a.ko".to_string(), with(&modinfo)),
                (MODULE_COMMON.to_string(), with("")),
            ]
            .into()
        };
        assert!(compare(&build("b,c"), &build("c,b")).clean());

        let mut c = compare(&build(""), &build("kunit"));
        assert_eq!(c.members["a.ko"], ["a.o", "a.mod.o", MODULE_COMMON]);
        let objects: Vec<&str> = c.differences.iter().map(|d| d.object.as_str()).collect();
        assert_eq!(objects, ["a.ko", "a.mod.o"]);
        assert!(report(&c, false).contains("| a.ko | .modinfo | depends= | depends=kunit |"));
        let rules = Divergences::parse(
            r#"
[[divergence]]
what = ".modinfo"
object = "a.mod.o"
only = "reference"
item = "depends="
reason = "a kunit call only rucc keeps"

[[divergence]]
what = ".modinfo"
object = "a.mod.o"
only = "other"
item = "depends=kunit"
reason = "a kunit call only rucc keeps"
"#,
        )
        .unwrap();
        rules.explain(&mut c);
        assert!(c.clean());
    }

    /// The members are read from the `.mod` file next to a module, leaving out the module itself,
    /// which is all a module of one file lists.
    #[test]
    fn a_module_has_the_members_its_mod_file_lists() {
        let dir = std::env::temp_dir().join(format!("rk-sections-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("fs/f2fs")).unwrap();
        std::fs::write(
            dir.join("fs/f2fs/f2fs.mod"),
            "fs/f2fs/dir.o\nfs/f2fs/inline.o\n",
        )
        .unwrap();
        std::fs::write(dir.join("fs/f2fs/one.mod"), "fs/f2fs/one.o\n").unwrap();
        let of = |name: &str| members(&dir.join(name), name);
        assert_eq!(of("fs/f2fs/f2fs.o"), ["fs/f2fs/dir.o", "fs/f2fs/inline.o"]);
        assert!(of("fs/f2fs/one.o").is_empty());
        assert!(of("fs/f2fs/dir.o").is_empty());
        assert!(of("fs/f2fs/f2fs.ko").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_site_with_its_condition_is_the_site_without_it() {
        let rules = Divergences::parse(
            "[[divergence]]\nwhat = \"__bug_table\"\nonly = \"reference\"\n\
             item = '4=\"\" 8=\"mm/x.c\" 1c000309'\nreason = \"r\"\n",
        )
        .unwrap();
        let d = plain("mm/x.o", "__bug_table", String::new(), String::new());
        let at = |item: &str| rules.rule(&d, Only::Reference, item).is_some();
        assert!(at("4=\"\" 8=\"mm/x.c\" 1c000309"));
        assert!(at("4=\"!p\" 8=\"mm/x.c\" 1c000309"));
        assert!(!at("4=\"!p\" 8=\"mm/x.c\" 1d000309"));
        assert_eq!(uncondition("4=\"\" 8=\"mm/x.c\" 1c000309"), None);
    }

    #[test]
    fn the_divergences_file_parses_and_every_rule_has_a_reason() {
        let rules = Divergences::parse(include_str!("../../../sections-divergences.toml")).unwrap();
        assert!(!rules.divergences.is_empty());
        assert!(Divergences::parse("[[divergence]]\nwhat = \"sections\"\nonly = \"either\"\nitem = \".x\"\nreason = \" \"\n").is_err());
    }

    /// An arm64 object with one bug entry whose file string sits at `at` in `.hyp.rodata`,
    /// reached through a local label as the nVHE objects reach it.
    fn hyp(at: usize) -> Vec<u8> {
        use object::write::{Symbol, SymbolSection};
        let mut b = Builder::aarch64();
        let f = b.function("f", &[0; 8]);
        let mut rodata = vec![7; at];
        rodata.extend_from_slice(b"arch/arm64/kvm/hyp/nvhe/mem_protect.c\0");
        let rodata = b.section(".hyp.rodata", &rodata);
        let label = b.obj.add_symbol(Symbol {
            name: b"__kvm_nvhe_.L14472\x021".to_vec(),
            value: at as u64,
            size: 0,
            kind: object::SymbolKind::Label,
            scope: object::SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(rodata),
            flags: object::SymbolFlags::None,
        });
        let bug = b.section(
            "__bug_table",
            &[0, 0, 0, 0, 0, 0, 0, 0, 0x99, 0x01, 0x01, 0x09],
        );
        b.reloc(bug, 0, f, object::elf::R_AARCH64_PREL32);
        b.reloc(bug, 4, label, object::elf::R_AARCH64_PREL32);
        b.bytes()
    }

    #[test]
    fn a_bug_entry_names_its_file_wherever_the_string_was_linked() {
        let one = read(&hyp(1027)).unwrap();
        let other = read(&hyp(1008)).unwrap();
        assert_eq!(
            one.distinct["__bug_table"],
            [r#"4="arch/arm64/kvm/hyp/nvhe/mem_protect.c" 99010109"#.to_string()].into()
        );
        assert_eq!(one.distinct, other.distinct);
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
