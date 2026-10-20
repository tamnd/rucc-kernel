//! `rk btf`: the BTF of two builds' vmlinux, compared.
//!
//! With `CONFIG_DEBUG_INFO_BTF` the kernel build runs pahole over the DWARF of vmlinux and links
//! what it says into a `.BTF` section, which BPF programs, `bpftool` and the verifier read. A
//! compiler whose DWARF pahole reads differently gives a kernel whose BTF describes other types,
//! and BPF programs built against the reference then fail to load. The plan's check (09, K5) is
//! that both builds describe the same types and the same functions, and that functions only one
//! of them has, because the compilers inlined differently, are listed and not failed.
//!
//! Type IDs differ between any two builds, so each type is written out by name instead: a named
//! struct, union, enum or typedef is referred to by its name, and an anonymous one is written in
//! full where it is used. Every named type then has a description, and the same name may have more
//! than one, since two files may define different structs of one name. Both builds must give each
//! name the same descriptions, and each function the same prototype.
//!
//! pahole merges the copies of a type that every file's DWARF gives it into one, and a type it could
//! not merge stays twice, with two IDs, even when both copies read the same here. The kernel looks
//! a type up by its name and gets one copy, so a struct whose member has the other copy's ID no
//! longer matches it. That is the warning the rucc kernel printed at boot for `tcp_congestion_ops`,
//! `Qdisc_ops` and `hid_bpf_ops`, that the second member of the `bpf_struct_ops_` value type should
//! be the type itself. So both builds must also have each name as many times.
//!
//! A function name more than one source file defines, such as a `__weak` default and the arch's
//! own version, or two static `m_next`s, has one prototype in the BTF however many definitions
//! there are. pahole keeps whichever it reads first, and with `-j` that is not the same from one
//! run to the next, so such a function's prototype is listed and not failed. The names come from
//! the DWARF in vmlinux.

use object::{Object as _, ObjectSection as _};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// How many lines of each list the report shows.
const SHOWN: usize = 40;

/// How deep a description goes before it stops, against a type that refers to itself without a
/// name in between, which well formed BTF does not have.
const DEEPEST: usize = 64;

/// What a build's BTF says, which `--save` keeps as JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Btf {
    /// Each named type's descriptions, by kind and name, such as `struct file`.
    pub types: BTreeMap<String, BTreeSet<String>>,
    /// Each function's prototypes, by name.
    pub functions: BTreeMap<String, BTreeSet<String>>,
    /// Functions more than one source file defines, whose prototype is whichever pahole read
    /// first.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub defined_twice: BTreeSet<String>,
    /// How many times each named type is in the section, for the names that are there more than
    /// once.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub copies: BTreeMap<String, usize>,
}

/// One type as it is in the section, with the words that follow it.
#[derive(Debug, Clone)]
struct Raw {
    name: String,
    kind: u32,
    vlen: usize,
    kind_flag: bool,
    size_or_type: u32,
    extra: Vec<u32>,
}

const INT: u32 = 1;
const PTR: u32 = 2;
const ARRAY: u32 = 3;
const STRUCT: u32 = 4;
const UNION: u32 = 5;
const ENUM: u32 = 6;
const FWD: u32 = 7;
const TYPEDEF: u32 = 8;
const VOLATILE: u32 = 9;
const CONST: u32 = 10;
const RESTRICT: u32 = 11;
const FUNC: u32 = 12;
const FUNC_PROTO: u32 = 13;
const VAR: u32 = 14;
const DATASEC: u32 = 15;
const FLOAT: u32 = 16;
const DECL_TAG: u32 = 17;
const TYPE_TAG: u32 = 18;
const ENUM64: u32 = 19;

/// Read the `.BTF` section of a vmlinux, or of the vmlinux in a build directory, or a file that is
/// BTF alone, such as `/sys/kernel/btf/vmlinux` or what `pahole --btf_encode_detached` writes.
pub fn scan(path: &Path) -> Result<Btf, String> {
    let file = if path.is_dir() {
        path.join("vmlinux")
    } else {
        path.to_path_buf()
    };
    let data = std::fs::read(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
    if is_raw(&data) {
        return parse(&data).map_err(|e| format!("{}: {e}", file.display()));
    }
    let elf = object::File::parse(&*data).map_err(|e| format!("{}: {e}", file.display()))?;
    let section = elf
        .section_by_name(".BTF")
        .ok_or_else(|| format!("{} has no .BTF section", file.display()))?;
    let bytes = section.data().map_err(|e| e.to_string())?;
    let mut btf = parse(bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    btf.defined_twice = defined_twice(&elf).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(btf)
}

/// The functions that more than one source file defines, going by the DWARF. An inline function
/// from a header is defined in every unit that uses it, but always in the one file, so it is not
/// one of them. A vmlinux without DWARF has none.
///
/// gas writes a function of an assembly file with `-gdwarf-5` and gives it no file, so there the
/// file is the unit's own. That is how an arch's `.S` that overrides a `__weak` C default, such as
/// `arch_bpf_timed_may_goto`, counts as a second definition, which pahole may read first and
/// describe as `static fn() -> void`.
fn defined_twice(elf: &object::File) -> Result<BTreeSet<String>, gimli::Error> {
    let endian = if elf.is_little_endian() {
        gimli::RunTimeEndian::Little
    } else {
        gimli::RunTimeEndian::Big
    };
    let load = |id: gimli::SectionId| -> Result<_, gimli::Error> {
        let data = elf.section_by_name(id.name()).and_then(|s| s.data().ok());
        Ok(gimli::EndianSlice::new(data.unwrap_or(&[]), endian))
    };
    let dwarf = gimli::Dwarf::load(load)?;
    let mut files: BTreeMap<String, BTreeSet<Vec<u8>>> = BTreeMap::new();
    let mut headers = dwarf.units();
    while let Some(header) = headers.next()? {
        let unit = dwarf.unit(header)?;
        let mut entries = unit.entries();
        let mut assembly = false;
        while let Some(entry) = entries.next_dfs()? {
            if entry.tag() == gimli::DW_TAG_compile_unit {
                assembly = matches!(
                    entry.attr_value(gimli::DW_AT_language),
                    Some(gimli::AttributeValue::Language(
                        gimli::DW_LANG_Mips_Assembler
                    ))
                );
            }
            if entry.tag() != gimli::DW_TAG_subprogram
                || entry.attr(gimli::DW_AT_declaration).is_some()
            {
                continue;
            }
            let Some(name) = entry.attr_value(gimli::DW_AT_name) else {
                continue;
            };
            let path = match entry.attr_value(gimli::DW_AT_decl_file) {
                Some(gimli::AttributeValue::FileIndex(index)) => file_path(&dwarf, &unit, index)?,
                _ if assembly => unit_path(&unit)?,
                _ => None,
            };
            let Some(path) = path else {
                continue;
            };
            let name = dwarf
                .attr_string(&unit, name)?
                .to_string_lossy()
                .into_owned();
            files.entry(name).or_default().insert(path);
        }
    }
    Ok(files
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(name, _)| name)
        .collect())
}

/// The directory and name of a file in a unit's line program, as the compiler wrote them.
fn file_path<R: gimli::Reader>(
    dwarf: &gimli::Dwarf<R>,
    unit: &gimli::Unit<R>,
    index: u64,
) -> Result<Option<Vec<u8>>, gimli::Error> {
    let Some(program) = &unit.line_program else {
        return Ok(None);
    };
    let header = program.header();
    let Some(file) = header.file(index) else {
        return Ok(None);
    };
    let mut path = Vec::new();
    if let Some(dir) = file.directory(header) {
        path.extend_from_slice(&dwarf.attr_string(unit, dir)?.to_slice()?);
        path.push(b'/');
    }
    path.extend_from_slice(&dwarf.attr_string(unit, file.path_name())?.to_slice()?);
    Ok(Some(path))
}

/// The directory and name of a unit's own file, as the compiler or assembler wrote them.
fn unit_path<R: gimli::Reader>(unit: &gimli::Unit<R>) -> Result<Option<Vec<u8>>, gimli::Error> {
    let Some(name) = &unit.name else {
        return Ok(None);
    };
    let mut path = Vec::new();
    if let Some(dir) = &unit.comp_dir {
        path.extend_from_slice(&dir.to_slice()?);
        path.push(b'/');
    }
    path.extend_from_slice(&name.to_slice()?);
    Ok(Some(path))
}

/// Whether a file is BTF alone, starting with the magic in either byte order.
#[must_use]
pub fn is_raw(data: &[u8]) -> bool {
    matches!(data.get(..2), Some([0x9f, 0xeb] | [0xeb, 0x9f]))
}

/// Read a `.BTF` section.
pub fn parse(bytes: &[u8]) -> Result<Btf, String> {
    let little = match bytes.get(..2) {
        Some([0x9f, 0xeb]) => true,
        Some([0xeb, 0x9f]) => false,
        _ => return Err("the .BTF section does not start with the BTF magic".to_string()),
    };
    let word = |at: usize| -> Result<u32, String> {
        let b: [u8; 4] = bytes
            .get(at..at + 4)
            .and_then(|b| b.try_into().ok())
            .ok_or("the .BTF section is cut short")?;
        Ok(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let header = word(4)? as usize;
    let (type_off, type_len) = (word(8)? as usize, word(12)? as usize);
    let (str_off, str_len) = (word(16)? as usize, word(20)? as usize);
    let strings = bytes
        .get(header + str_off..header + str_off + str_len)
        .ok_or("the string table is outside the section")?;
    let name = |off: u32| -> String {
        let rest = strings.get(off as usize..).unwrap_or_default();
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        String::from_utf8_lossy(&rest[..end]).into_owned()
    };
    let mut types = Vec::new();
    let (mut at, end) = (header + type_off, header + type_off + type_len);
    while at < end {
        let info = word(at + 4)?;
        let kind = (info >> 24) & 0x1f;
        let vlen = (info & 0xffff) as usize;
        let words = match kind {
            INT | VAR | DECL_TAG => 1,
            ARRAY => 3,
            STRUCT | UNION | DATASEC | ENUM64 => 3 * vlen,
            ENUM | FUNC_PROTO => 2 * vlen,
            PTR | FWD | TYPEDEF | VOLATILE | CONST | RESTRICT | FUNC | FLOAT | TYPE_TAG => 0,
            _ => {
                return Err(format!(
                    "type {} has the unknown kind {kind}",
                    types.len() + 1
                ));
            }
        };
        let extra = (0..words)
            .map(|i| word(at + 12 + 4 * i))
            .collect::<Result<Vec<_>, _>>()?;
        types.push(Raw {
            name: name(word(at)?),
            kind,
            vlen,
            kind_flag: info >> 31 == 1,
            size_or_type: word(at + 8)?,
            extra,
        });
        at += 12 + 4 * words;
    }
    Ok(Table { types, name: &name }.describe())
}

/// The types of a section, numbered from 1, as type 0 is `void`.
struct Table<'a> {
    types: Vec<Raw>,
    name: &'a dyn Fn(u32) -> String,
}

impl Table<'_> {
    fn get(&self, id: u32) -> Option<&Raw> {
        self.types.get((id as usize).checked_sub(1)?)
    }

    /// How a use of a type is written: by name when it has one, in full when not.
    fn refer(&self, id: u32, depth: usize) -> String {
        if id == 0 {
            return "void".to_string();
        }
        if depth > DEEPEST {
            return "...".to_string();
        }
        let Some(t) = self.get(id) else {
            return format!("<missing {id}>");
        };
        let next = |id| self.refer(id, depth + 1);
        match t.kind {
            PTR => format!("*{}", next(t.size_or_type)),
            ARRAY => format!("[{}]{}", t.extra[2], next(t.extra[0])),
            CONST | VOLATILE | RESTRICT => {
                // gcc writes `const volatile` as volatile over const and rucc the other way, which
                // says the same, so a run of qualifiers is written in one order.
                let mut words = Vec::new();
                let mut under = id;
                while let Some(q) = self.get(under)
                    && matches!(q.kind, CONST | VOLATILE | RESTRICT)
                {
                    words.push(match q.kind {
                        CONST => "const",
                        VOLATILE => "volatile",
                        _ => "restrict",
                    });
                    under = q.size_or_type;
                }
                words.sort_unstable();
                words.dedup();
                format!("{} {}", words.join(" "), next(under))
            }
            TYPE_TAG => format!("tag({}) {}", t.name, next(t.size_or_type)),
            FUNC_PROTO => self.proto(t, depth),
            STRUCT | UNION | ENUM | ENUM64 if t.name.is_empty() => self.body(t, depth),
            STRUCT | ENUM | ENUM64 | UNION | FWD => format!("{} {}", Self::tag(t), t.name),
            _ => t.name.clone(),
        }
    }

    /// The word a struct, union, enum or forward declaration is named with.
    fn tag(t: &Raw) -> &'static str {
        match t.kind {
            UNION => "union",
            FWD if t.kind_flag => "union",
            ENUM | ENUM64 => "enum",
            _ => "struct",
        }
    }

    /// A struct, union or enum written in full.
    fn body(&self, t: &Raw, depth: usize) -> String {
        let mut s = format!("{} size {} {{", Self::tag(t), t.size_or_type);
        match t.kind {
            STRUCT | UNION => {
                for m in t.extra.chunks(3) {
                    let (bits, at) = if t.kind_flag {
                        (m[2] >> 24, m[2] & 0x00ff_ffff)
                    } else {
                        (0, m[2])
                    };
                    let _ = write!(
                        s,
                        " {}: {} @{at}",
                        (self.name)(m[0]),
                        self.refer(m[1], depth + 1)
                    );
                    if bits > 0 {
                        let _ = write!(s, ":{bits}");
                    }
                    s.push(';');
                }
            }
            ENUM => {
                for v in t.extra.chunks(2) {
                    let value = if t.kind_flag {
                        i64::from(v[1].cast_signed()).to_string()
                    } else {
                        v[1].to_string()
                    };
                    let _ = write!(s, " {} = {value};", (self.name)(v[0]));
                }
            }
            _ => {
                for v in t.extra.chunks(3) {
                    let value = u64::from(v[2]) << 32 | u64::from(v[1]);
                    let value = if t.kind_flag {
                        value.cast_signed().to_string()
                    } else {
                        value.to_string()
                    };
                    let _ = write!(s, " {} = {value};", (self.name)(v[0]));
                }
            }
        }
        s.push_str(" }");
        s
    }

    /// A function prototype, with its parameters' names, which come from the DWARF.
    fn proto(&self, t: &Raw, depth: usize) -> String {
        let params: Vec<String> = t
            .extra
            .chunks(2)
            .map(|p| match (p[1], (self.name)(p[0])) {
                (0, _) => "...".to_string(),
                (ty, name) if name.is_empty() => self.refer(ty, depth + 1),
                (ty, name) => format!("{name}: {}", self.refer(ty, depth + 1)),
            })
            .collect();
        format!(
            "fn({}) -> {}",
            params.join(", "),
            self.refer(t.size_or_type, depth + 1)
        )
    }

    /// Every named type and function, described.
    fn describe(&self) -> Btf {
        let mut btf = Btf::default();
        for t in &self.types {
            if t.name.is_empty() {
                continue;
            }
            let (key, description) = match t.kind {
                STRUCT | UNION | ENUM | ENUM64 => {
                    (format!("{} {}", Self::tag(t), t.name), self.body(t, 0))
                }
                TYPEDEF => (format!("typedef {}", t.name), self.refer(t.size_or_type, 0)),
                INT => {
                    let e = t.extra[0];
                    let sign = match e >> 24 {
                        1 => " signed",
                        2 => " char",
                        4 => " bool",
                        _ => "",
                    };
                    let shape = format!("size {}{sign} bits {}", t.size_or_type, e & 0xff);
                    (format!("int {}", t.name), shape)
                }
                FLOAT => (
                    format!("float {}", t.name),
                    format!("size {}", t.size_or_type),
                ),
                VAR => (
                    format!("var {}", t.name),
                    format!("{} linkage {}", self.refer(t.size_or_type, 0), t.extra[0]),
                ),
                DECL_TAG => (
                    format!("decl_tag {}", t.name),
                    format!(
                        "{} component {}",
                        self.refer(t.size_or_type, 0),
                        t.extra[0].cast_signed()
                    ),
                ),
                FUNC => {
                    let proto = self.refer(t.size_or_type, 0);
                    let linkage = ["static", "global", "extern"]
                        .get(t.vlen)
                        .unwrap_or(&"linkage?");
                    btf.functions
                        .entry(t.name.clone())
                        .or_default()
                        .insert(format!("{linkage} {proto}"));
                    continue;
                }
                _ => continue,
            };
            *btf.copies.entry(key.clone()).or_default() += 1;
            btf.types.entry(key).or_default().insert(description);
        }
        btf.copies.retain(|_, n| *n > 1);
        btf
    }
}

/// What differs between two builds' BTF.
#[derive(Debug, Default)]
pub struct Comparison {
    /// Named types both builds have and describe differently, with each build's descriptions.
    pub differ: Vec<(String, BTreeSet<String>, BTreeSet<String>)>,
    /// Named types only the reference has.
    pub reference_only: Vec<String>,
    /// Named types only the other build has.
    pub other_only: Vec<String>,
    /// Named types both builds have, but not as many times, with how many times each has it.
    pub copies: Vec<(String, usize, usize)>,
    /// Functions both builds have with different prototypes.
    pub prototypes: Vec<(String, BTreeSet<String>, BTreeSet<String>)>,
    /// Functions with different prototypes that more than one file defines, which is not a
    /// finding since pahole keeps whichever definition it reads first.
    pub picked: Vec<(String, BTreeSet<String>, BTreeSet<String>)>,
    /// Functions only the reference has, which is not a finding.
    pub functions_reference_only: Vec<String>,
    /// Functions only the other build has, which is not a finding.
    pub functions_other_only: Vec<String>,
    /// Named types and functions in each build.
    pub counts: ((usize, usize), (usize, usize)),
}

impl Comparison {
    /// Whether both builds describe the same types and give shared functions the same prototype.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.differ.is_empty()
            && self.reference_only.is_empty()
            && self.other_only.is_empty()
            && self.copies.is_empty()
            && self.prototypes.is_empty()
    }
}

type Differ = Vec<(String, BTreeSet<String>, BTreeSet<String>)>;

fn split(
    reference: &BTreeMap<String, BTreeSet<String>>,
    other: &BTreeMap<String, BTreeSet<String>>,
) -> (Differ, Vec<String>, Vec<String>) {
    let mut differ = Vec::new();
    let mut reference_only = Vec::new();
    for (name, r) in reference {
        match other.get(name) {
            Some(o) if o != r => differ.push((name.clone(), r.clone(), o.clone())),
            Some(_) => {}
            None => reference_only.push(name.clone()),
        }
    }
    let other_only = other
        .keys()
        .filter(|name| !reference.contains_key(*name))
        .cloned()
        .collect();
    (differ, reference_only, other_only)
}

/// Compare the other build's BTF with the reference's.
#[must_use]
pub fn compare(reference: &Btf, other: &Btf) -> Comparison {
    let (differ, reference_only, other_only) = split(&reference.types, &other.types);
    let (prototypes, functions_reference_only, functions_other_only) =
        split(&reference.functions, &other.functions);
    let twice =
        |name: &str| reference.defined_twice.contains(name) || other.defined_twice.contains(name);
    let (picked, prototypes) = prototypes.into_iter().partition(|(name, _, _)| twice(name));
    let times = |btf: &Btf, name: &str| btf.copies.get(name).copied().unwrap_or(1);
    let copies = reference
        .copies
        .keys()
        .chain(other.copies.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|name| reference.types.contains_key(*name) && other.types.contains_key(*name))
        .map(|name| (name.clone(), times(reference, name), times(other, name)))
        .filter(|(_, r, o)| r != o)
        .collect();
    Comparison {
        differ,
        reference_only,
        other_only,
        copies,
        prototypes,
        picked,
        functions_reference_only,
        functions_other_only,
        counts: (
            (reference.types.len(), reference.functions.len()),
            (other.types.len(), other.functions.len()),
        ),
    }
}

fn list(s: &mut String, title: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let _ = writeln!(s, "\n{title} ({}):\n", names.len());
    for name in names.iter().take(SHOWN) {
        let _ = writeln!(s, "- `{name}`");
    }
    if names.len() > SHOWN {
        let _ = writeln!(s, "- and {} more", names.len() - SHOWN);
    }
}

fn pairs(s: &mut String, title: &str, differ: &Differ) {
    if differ.is_empty() {
        return;
    }
    let _ = writeln!(s, "\n{title} ({}):\n", differ.len());
    for (name, r, o) in differ.iter().take(SHOWN) {
        let _ = writeln!(s, "- `{name}`");
        for d in r.difference(o) {
            let _ = writeln!(s, "  - reference: `{d}`");
        }
        for d in o.difference(r) {
            let _ = writeln!(s, "  - other: `{d}`");
        }
    }
    if differ.len() > SHOWN {
        let _ = writeln!(s, "- and {} more", differ.len() - SHOWN);
    }
}

/// The comparison as Markdown.
#[must_use]
pub fn report(c: &Comparison) -> String {
    let mut s = String::from("### btf\n\n");
    let ((rt, rf), (ot, of)) = c.counts;
    let _ = writeln!(
        s,
        "The reference describes {rt} named types and {rf} functions, the other build {ot} and {of}."
    );
    pairs(&mut s, "Types described differently", &c.differ);
    list(&mut s, "Types only the reference has", &c.reference_only);
    list(&mut s, "Types only the other build has", &c.other_only);
    if !c.copies.is_empty() {
        let _ = writeln!(
            s,
            "\nTypes the builds have a different number of times ({}):\n",
            c.copies.len()
        );
        for (name, r, o) in c.copies.iter().take(SHOWN) {
            let _ = writeln!(
                s,
                "- `{name}`: {r} in the reference, {o} in the other build"
            );
        }
        if c.copies.len() > SHOWN {
            let _ = writeln!(s, "- and {} more", c.copies.len() - SHOWN);
        }
    }
    pairs(&mut s, "Functions with different prototypes", &c.prototypes);
    pairs(
        &mut s,
        "Functions more than one file defines, with different prototypes, not failed since pahole keeps whichever it reads first",
        &c.picked,
    );
    list(
        &mut s,
        "Functions only the reference has, not failed since inlining differs",
        &c.functions_reference_only,
    );
    list(
        &mut s,
        "Functions only the other build has, not failed since inlining differs",
        &c.functions_other_only,
    );
    if c.clean() {
        s.push_str("\nBoth builds describe the same types, and shared functions the same way.\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A section built by hand from types given as words and a string table.
    fn section(types: &[u32], strings: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0xeb9f_u16.to_le_bytes());
        out.extend_from_slice(&[1, 0]);
        let type_len = u32::try_from(types.len() * 4).unwrap();
        let str_len = u32::try_from(strings.len()).unwrap();
        for w in [24, 0, type_len, type_len, str_len] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        for w in types {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend_from_slice(strings);
        out
    }

    fn info(kind: u32, vlen: u32) -> u32 {
        kind << 24 | vlen
    }

    /// Strings: 1 int, 5 point, 11 x, 13 y, 15 f, 17 p, 19 u.
    const STRINGS: &[u8] = b"\0int\0point\0x\0y\0f\0p\0u\0";

    /// `int`, `struct point { int x; int y; }`, `struct point *`, `int f(struct point *p)`, and a
    /// typedef `u` of an anonymous struct holding a point, laid out in the order given.
    fn sample(order_swapped: bool) -> Vec<u8> {
        let int = [1, info(INT, 0), 4, 1 << 24 | 32];
        let (point, ptr) = if order_swapped { (3, 2) } else { (2, 3) };
        let point_words = [5, info(STRUCT, 2), 8, 11, 1, 0, 13, 1, 32];
        let ptr_words = [0, info(PTR, 0), point];
        let mut words = int.to_vec();
        if order_swapped {
            words.extend_from_slice(&ptr_words);
            words.extend_from_slice(&point_words);
        } else {
            words.extend_from_slice(&point_words);
            words.extend_from_slice(&ptr_words);
        }
        words.extend_from_slice(&[0, info(FUNC_PROTO, 1), 1, 17, ptr]);
        words.extend_from_slice(&[15, info(FUNC, 1), 4]);
        words.extend_from_slice(&[0, info(STRUCT, 1), 8, 17, point, 0]);
        words.extend_from_slice(&[19, info(TYPEDEF, 0), 6]);
        section(&words, STRINGS)
    }

    #[test]
    fn types_are_described_by_name_whatever_their_ids() {
        let a = parse(&sample(false)).unwrap();
        let b = parse(&sample(true)).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a.types["struct point"].iter().next().unwrap(),
            "struct size 8 { x: int @0; y: int @32; }"
        );
        assert_eq!(
            a.functions["f"].iter().next().unwrap(),
            "global fn(p: *struct point) -> int"
        );
        assert_eq!(
            a.types["typedef u"].iter().next().unwrap(),
            "struct size 8 { p: struct point @0; }"
        );
        assert!(compare(&a, &b).clean());
    }

    #[test]
    fn a_member_moved_is_a_finding_and_a_function_missing_is_not() {
        let a = parse(&sample(false)).unwrap();
        let mut b = a.clone();
        b.types.insert(
            "struct point".to_string(),
            BTreeSet::from(["struct size 8 { x: int @0; y: int @16; }".to_string()]),
        );
        b.functions.remove("f");
        let c = compare(&a, &b);
        assert!(!c.clean());
        assert_eq!(c.differ.len(), 1);
        assert_eq!(c.functions_reference_only, ["f"]);
        let mut d = a.clone();
        d.functions.remove("f");
        let c = compare(&a, &d);
        assert!(c.clean(), "{}", report(&c));
        assert!(report(&c).contains("- `f`"));
    }

    #[test]
    fn a_function_two_files_define_may_have_either_prototype() {
        let a = parse(&sample(false)).unwrap();
        let mut b = a.clone();
        b.functions.insert(
            "f".to_string(),
            BTreeSet::from(["static fn(q: *struct point) -> int".to_string()]),
        );
        assert!(
            !compare(&a, &b).clean(),
            "one definition, so the prototype is a finding"
        );
        b.defined_twice.insert("f".to_string());
        let c = compare(&a, &b);
        assert!(c.clean(), "{}", report(&c));
        assert_eq!(c.picked.len(), 1);
        assert!(report(&c).contains("pahole keeps whichever it reads first"));
    }

    #[test]
    fn a_type_pahole_could_not_merge_is_a_finding() {
        let mut words = sample(false);
        let a = parse(&words).unwrap();
        assert!(a.copies.is_empty());
        let header = 24;
        let type_len = u32::from_le_bytes(words[12..16].try_into().unwrap()) as usize;
        let point = [5, info(STRUCT, 2), 8, 11, 1, 0, 13, 1, 32];
        let extra: Vec<u8> = point.iter().flat_map(|w: &u32| w.to_le_bytes()).collect();
        words.splice(header + type_len..header + type_len, extra.iter().copied());
        let grown = u32::try_from(type_len + extra.len()).unwrap();
        words[12..16].copy_from_slice(&grown.to_le_bytes());
        words[16..20].copy_from_slice(&grown.to_le_bytes());
        let b = parse(&words).unwrap();
        assert_eq!(a.types, b.types, "both copies read the same");
        assert_eq!(b.copies["struct point"], 2);
        let c = compare(&a, &b);
        assert!(!c.clean());
        assert_eq!(c.copies, [("struct point".to_string(), 1, 2)]);
        assert!(report(&c).contains("- `struct point`: 1 in the reference, 2 in the other build"));
        assert!(compare(&b, &b).clean());
    }

    /// An ELF holding only DWARF 4 units, each a file in a language with the functions it
    /// defines, none given a file of its own, the way gas writes a file of assembly.
    fn units(each: &[(&str, u16, &[&str])]) -> Vec<u8> {
        use object::write::Object;
        let abbrev = [
            1, 0x11, 1, 0x03, 0x08, 0x1b, 0x08, 0x13, 0x05, 0, 0, 2, 0x2e, 0, 0x03, 0x08, 0, 0, 0,
        ];
        let mut info = Vec::new();
        for (file, language, functions) in each {
            let mut body = vec![1];
            body.extend_from_slice(file.as_bytes());
            body.extend_from_slice(b"\0/k\0");
            body.extend_from_slice(&language.to_le_bytes());
            for function in *functions {
                body.push(2);
                body.extend_from_slice(function.as_bytes());
                body.push(0);
            }
            body.push(0);
            let length = u32::try_from(body.len() + 7).unwrap();
            info.extend_from_slice(&length.to_le_bytes());
            info.extend_from_slice(&[4, 0, 0, 0, 0, 0, 8]);
            info.extend_from_slice(&body);
        }
        let mut elf = Object::new(
            object::BinaryFormat::Elf,
            object::Architecture::Aarch64,
            object::Endianness::Little,
        );
        for (name, data) in [(".debug_abbrev", abbrev.to_vec()), (".debug_info", info)] {
            let id = elf.add_section(
                Vec::new(),
                name.as_bytes().to_vec(),
                object::SectionKind::Debug,
            );
            elf.append_section_data(id, &data, 1);
        }
        elf.write().unwrap()
    }

    #[test]
    fn a_function_an_assembly_file_defines_is_defined_in_that_file() {
        const ASSEMBLY: u16 = 0x8001;
        const C: u16 = 0x0c;
        let twice = |each: &[(&str, u16, &[&str])]| {
            let bytes = units(each);
            defined_twice(&object::File::parse(&*bytes).unwrap()).unwrap()
        };
        let both = twice(&[("a.S", ASSEMBLY, &["f", "g"]), ("b.S", ASSEMBLY, &["f"])]);
        assert_eq!(both, BTreeSet::from(["f".to_string()]));
        let unplaced = twice(&[("a.S", ASSEMBLY, &["f"]), ("c.c", C, &["f"])]);
        assert!(
            unplaced.is_empty(),
            "a C function with no file is not placed: {unplaced:?}"
        );
    }

    #[test]
    fn qualifiers_are_written_in_one_order() {
        let int = [1, info(INT, 0), 4, 1 << 24 | 32];
        let cv = |first, second| {
            let mut words = int.to_vec();
            words.extend_from_slice(&[0, info(first, 0), 1, 0, info(second, 0), 2]);
            words.extend_from_slice(&[19, info(TYPEDEF, 0), 3]);
            parse(&section(&words, STRINGS)).unwrap()
        };
        let a = cv(CONST, VOLATILE);
        assert_eq!(a, cv(VOLATILE, CONST));
        assert_eq!(
            a.types["typedef u"].iter().next().unwrap(),
            "const volatile int"
        );
    }

    #[test]
    fn a_file_of_btf_alone_is_read_as_is() {
        let dir = std::env::temp_dir().join(format!("rk-btf-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vmlinux.btf");
        std::fs::write(&path, sample(false)).unwrap();
        let read = scan(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read.unwrap(), parse(&sample(false)).unwrap());
    }

    #[test]
    fn a_section_without_the_magic_is_refused() {
        assert!(parse(b"\0\0\0\0").is_err());
    }
}
