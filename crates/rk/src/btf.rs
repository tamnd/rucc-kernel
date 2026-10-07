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

/// Read the `.BTF` section of a vmlinux, or of the vmlinux in a build directory.
pub fn scan(path: &Path) -> Result<Btf, String> {
    let file = if path.is_dir() {
        path.join("vmlinux")
    } else {
        path.to_path_buf()
    };
    let data = std::fs::read(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
    let elf = object::File::parse(&*data).map_err(|e| format!("{}: {e}", file.display()))?;
    let section = elf
        .section_by_name(".BTF")
        .ok_or_else(|| format!("{} has no .BTF section", file.display()))?;
    let bytes = section.data().map_err(|e| e.to_string())?;
    parse(bytes).map_err(|e| format!("{}: {e}", file.display()))
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
            CONST => format!("const {}", next(t.size_or_type)),
            VOLATILE => format!("volatile {}", next(t.size_or_type)),
            RESTRICT => format!("restrict {}", next(t.size_or_type)),
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
            btf.types.entry(key).or_default().insert(description);
        }
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
    /// Functions both builds have with different prototypes.
    pub prototypes: Vec<(String, BTreeSet<String>, BTreeSet<String>)>,
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
    Comparison {
        differ,
        reference_only,
        other_only,
        prototypes,
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
    pairs(&mut s, "Functions with different prototypes", &c.prototypes);
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
    fn a_section_without_the_magic_is_refused() {
        assert!(parse(b"\0\0\0\0").is_err());
    }
}
