//! `rk modules-audit`: every module of a build is one the kernel's loader will take.
//!
//! The module loader applies relocations itself, and it knows only the few types GCC's kernel
//! code model produces. A compiler that reaches a global through the GOT, or emits a relocation
//! the loader has no case for, builds a module that fails `insmod` with "Unknown rela relocation"
//! (plan 9.7). So this lists the relocation types of every `.ko`, in the sections the loader
//! relocates, which are the allocated ones, and fails on any type outside the loader's list.
//!
//! It also checks that every module carries the build's `vermagic` and that the CRC each module
//! records for a symbol it imports, in `__versions`, is the CRC `Module.symvers` has for it.
//!
//! The accepted list is x86-64's, which has been the same since 2.6 apart from `PLT32` in 4.16
//! and `PC64` in 4.x. The GOTPCREL family is not on it: kernels before 5.x reject it outright and
//! the ones after reject it for modules built without PIE. Modules of other machines are listed
//! as not audited until their rows arrive.

use crate::objects;
use crate::symvers::Export;
use object::elf::{
    R_X86_64_32, R_X86_64_32S, R_X86_64_64, R_X86_64_NONE, R_X86_64_PC32, R_X86_64_PC64,
    R_X86_64_PLT32, RelocationType,
};
use object::{Object as _, ObjectSection as _, RelocationFlags, SectionFlags};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// The relocation types the x86-64 loader applies.
const X86_64_ACCEPTED: &[RelocationType] = &[
    R_X86_64_NONE,
    R_X86_64_64,
    R_X86_64_32,
    R_X86_64_32S,
    R_X86_64_PC32,
    R_X86_64_PLT32,
    R_X86_64_PC64,
];

/// The name of an x86-64 relocation type.
#[must_use]
pub fn type_name(r_type: RelocationType) -> String {
    let name = match r_type.0 {
        0 => "NONE",
        1 => "64",
        2 => "PC32",
        3 => "GOT32",
        4 => "PLT32",
        9 => "GOTPCREL",
        10 => "32",
        11 => "32S",
        24 => "PC64",
        26 => "GOTPC32",
        41 => "GOTPCRELX",
        42 => "REX_GOTPCRELX",
        n => return format!("R_X86_64_{n}"),
    };
    format!("R_X86_64_{name}")
}

/// What one module holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Module {
    /// Whether it is x86-64, the only machine audited so far.
    pub audited: bool,
    /// Relocations in allocated sections by type.
    pub relocs: BTreeMap<RelocationType, u64>,
    /// The `vermagic=` string of `.modinfo`.
    pub vermagic: String,
    /// The CRCs of `__versions` by symbol.
    pub versions: BTreeMap<String, u64>,
}

/// Read the `__versions` table: a CRC as an `unsigned long` and the name in the rest of a
/// 64-byte entry.
fn versions(data: &[u8]) -> BTreeMap<String, u64> {
    data.as_chunks::<64>()
        .0
        .iter()
        .map(|entry| {
            let crc = u64::from_le_bytes(entry[..8].try_into().unwrap_or_default());
            let name = entry[8..].split(|b| *b == 0).next().unwrap_or_default();
            (String::from_utf8_lossy(name).into_owned(), crc)
        })
        .collect()
}

/// Read one module.
pub fn read(data: &[u8]) -> Result<Module, String> {
    let file = object::File::parse(data).map_err(|e| e.to_string())?;
    let mut m = Module {
        audited: file.architecture() == object::Architecture::X86_64,
        ..Module::default()
    };
    for section in file.sections() {
        let name = section.name().unwrap_or_default();
        let allocated = matches!(section.flags(), SectionFlags::Elf { sh_flags, .. }
            if sh_flags.0 & object::elf::SHF_ALLOC.0 != 0);
        if allocated {
            for (_, reloc) in section.relocations() {
                if let RelocationFlags::Elf { r_type } = reloc.flags() {
                    *m.relocs.entry(r_type).or_default() += 1;
                }
            }
        }
        let data = section.data().map_err(|e| e.to_string())?;
        if name == ".modinfo" {
            for s in data.split(|b| *b == 0) {
                if let Some(v) = s.strip_prefix(b"vermagic=") {
                    m.vermagic = String::from_utf8_lossy(v).into_owned();
                }
            }
        } else if name == "__versions" {
            m.versions = versions(data);
        }
    }
    Ok(m)
}

/// What is wrong with a module, empty when nothing is.
#[must_use]
pub fn problems(m: &Module, release: &str, symvers: &BTreeMap<String, Export>) -> Vec<String> {
    let mut out = Vec::new();
    if !m.audited {
        return out;
    }
    for (r_type, n) in &m.relocs {
        if !X86_64_ACCEPTED.contains(r_type) {
            out.push(format!("{n} {} relocations", type_name(*r_type)));
        }
    }
    if m.vermagic.is_empty() {
        out.push("no vermagic".to_string());
    } else if !release.is_empty() && m.vermagic.split(' ').next() != Some(release) {
        out.push(format!("vermagic {} is not for {release}", m.vermagic));
    }
    for (symbol, crc) in &m.versions {
        let Some(export) = symvers.get(symbol) else {
            continue;
        };
        let wanted = u64::from_str_radix(export.crc.trim_start_matches("0x"), 16).ok();
        if wanted.is_some_and(|w| w != *crc & 0xffff_ffff) {
            out.push(format!(
                "{symbol} imported with CRC {crc:#010x}, exported with {}",
                export.crc
            ));
        }
    }
    out
}

/// Every module of a build directory with its problems, by relative path.
pub fn audit(out: &Path) -> Result<BTreeMap<String, (Module, Vec<String>)>, String> {
    let release = std::fs::read_to_string(out.join("include/config/kernel.release"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let (symvers, _) = crate::symvers::load(out);
    let mut audited = BTreeMap::new();
    for (name, path) in objects::find(out)? {
        if !objects::is_module(&name) {
            continue;
        }
        let data = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let m = read(&data).map_err(|e| format!("{name}: {e}"))?;
        let p = problems(&m, &release, &symvers);
        audited.insert(name, (m, p));
    }
    Ok(audited)
}

/// The audit as markdown.
#[must_use]
pub fn report(audited: &BTreeMap<String, (Module, Vec<String>)>) -> String {
    let mut s = String::from("### Modules\n\n");
    if audited.is_empty() {
        s.push_str("The build has no modules.\n");
        return s;
    }
    let mut total: BTreeMap<RelocationType, u64> = BTreeMap::new();
    for (m, _) in audited.values().filter(|(m, _)| m.audited) {
        for (t, n) in &m.relocs {
            *total.entry(*t).or_default() += n;
        }
    }
    let skipped = audited.values().filter(|(m, _)| !m.audited).count();
    let bad = audited.values().filter(|(_, p)| !p.is_empty()).count();
    let _ = writeln!(
        s,
        "{} modules, {bad} with problems, {skipped} of a machine not audited yet.\n",
        audited.len()
    );
    s.push_str("| relocation | count | loader |\n|---|---|---|\n");
    for (t, n) in &total {
        let verdict = if X86_64_ACCEPTED.contains(t) {
            "accepted"
        } else {
            "rejected"
        };
        let _ = writeln!(s, "| {} | {n} | {verdict} |", type_name(*t));
    }
    if bad > 0 {
        s.push_str("\n| module | problems |\n|---|---|\n");
        for (name, (_, p)) in audited.iter().filter(|(_, (_, p))| !p.is_empty()) {
            let _ = writeln!(s, "| {name} | {} |", p.join("; "));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::fixture::Builder;
    use object::elf::R_X86_64_REX_GOTPCRELX;

    fn module(r_type: RelocationType, vermagic: &str, crc: u64) -> Vec<u8> {
        let mut b = Builder::new();
        let f = b.function("init_module", &[0xc3]);
        let text = b.obj.section_id(object::write::StandardSection::Text);
        b.function("call", &[0xe8, 0, 0, 0, 0]);
        b.reloc(text, 2, f, r_type);
        b.section(
            ".modinfo",
            format!("license=GPL\0vermagic={vermagic}\0").as_bytes(),
        );
        let mut entry = crc.to_le_bytes().to_vec();
        entry.extend(b"printk");
        entry.resize(64, 0);
        b.section("__versions", &entry);
        b.bytes()
    }

    fn symvers() -> BTreeMap<String, Export> {
        crate::symvers::parse_symvers("0x12345678\tprintk\tvmlinux\tEXPORT_SYMBOL\n")
    }

    #[test]
    fn a_clean_module_passes() {
        let m = read(&module(R_X86_64_PLT32, "7.2.8 SMP mod_unload", 0x1234_5678)).unwrap();
        assert!(m.audited);
        assert_eq!(m.relocs[&R_X86_64_PLT32], 1);
        assert_eq!(m.versions["printk"], 0x1234_5678);
        assert!(problems(&m, "7.2.8", &symvers()).is_empty());
    }

    #[test]
    fn a_got_relocation_a_wrong_vermagic_and_a_wrong_crc_fail() {
        let m = read(&module(R_X86_64_REX_GOTPCRELX, "7.2.7 SMP", 0x1)).unwrap();
        let p = problems(&m, "7.2.8", &symvers());
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(p[0].contains("R_X86_64_REX_GOTPCRELX"));
        assert!(p[1].contains("vermagic"));
        assert!(p[2].contains("printk"));
        let text = report(&[("a.ko".to_string(), (m, p))].into());
        assert!(text.contains("| R_X86_64_REX_GOTPCRELX | 1 | rejected |"));
    }
}
