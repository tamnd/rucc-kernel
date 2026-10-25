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
//! the ones after reject it for modules built without PIE. The relocations of other machines'
//! modules are listed as not audited until their rows arrive, and their `vermagic` and CRCs are
//! checked all the same.

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
    /// Whether its relocations are audited, which is when it is x86-64, the only machine whose
    /// loader's list is here so far.
    pub audited: bool,
    /// Relocations in allocated sections by type.
    pub relocs: BTreeMap<RelocationType, u64>,
    /// The `vermagic=` string of `.modinfo`.
    pub vermagic: String,
    /// The CRCs of `__versions` by symbol.
    pub versions: BTreeMap<String, u64>,
}

/// Read the `__versions` table: a CRC as an `unsigned long`, eight bytes when `wide` and four
/// when not, and the name in the rest of a 64-byte entry.
fn versions(data: &[u8], wide: bool) -> BTreeMap<String, u64> {
    let width = if wide { 8 } else { 4 };
    data.as_chunks::<64>()
        .0
        .iter()
        .map(|entry| {
            let mut crc = [0; 8];
            crc[..width].copy_from_slice(&entry[..width]);
            let name = entry[width..].split(|b| *b == 0).next().unwrap_or_default();
            (
                String::from_utf8_lossy(name).into_owned(),
                u64::from_le_bytes(crc),
            )
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
            m.versions = versions(data, file.is_64());
        }
    }
    Ok(m)
}

/// A module without its debug information, which the loader never reads.
///
/// A distribution config builds thousands of modules with DWARF in them, and the kunit boot's
/// initramfs went over the 2 GiB QEMU reads of an initrd. Each `.debug_` section, and each
/// relocation section that applies to one, becomes an empty `SHT_NOBITS` section with nothing in
/// the file, so no section changes its index and no symbol or relocation has to be renumbered.
/// The loader lays out only allocated sections and relocates only those, so it does not notice.
/// The rest of the file is kept as it was, in the same order. A file that is not a little-endian
/// ELF relocatable object comes back unchanged, and so does a signed one, whose signature would
/// no longer match.
#[must_use]
pub fn without_debug(data: &[u8]) -> Vec<u8> {
    stripped(data, false).unwrap_or_else(|| data.to_vec())
}

/// A module without its debug information and without its `.BTF`, for a kernel it was not built
/// with.
///
/// A module's BTF is split BTF, whose type IDs carry on from the ones of the `vmlinux` it was
/// built against, so in any other kernel they name other types. The kernel cannot tell when the
/// two happen to have the same number of types and strings, and then a module that registers
/// kfuncs fails to load. On a 7.2.8 arm64 nightly GCC's `tcp_dctcp.ko` failed in the rucc
/// kernel, which said its kfunc was `usb_of_get_connect_type` and already in `vmlinux`, and it
/// loaded on another run of the same night. Without the section the kernel says "missing module BTF" and
/// loads the module, which is what it already did with the BTF it could not read.
#[must_use]
pub fn without_debug_or_btf(data: &[u8]) -> Vec<u8> {
    stripped(data, true).unwrap_or_else(|| data.to_vec())
}

/// One section header, the fields of it that are read or written here.
struct Header {
    at: usize,
    name: u32,
    kind: u32,
    flags: u64,
    offset: u64,
    size: u64,
    info: u32,
    align: u64,
}

/// Where the section headers of an ELF file are and what they say.
struct Layout {
    /// ELF64 rather than ELF32.
    wide: bool,
    /// Where `e_shoff` is in the ELF header, and how long the header is.
    shoff_at: usize,
    ehsize: usize,
    /// Where the section header table is, and how long one entry is.
    shoff: usize,
    entsize: usize,
    headers: Vec<Header>,
    /// The section names.
    shstrndx: usize,
}

impl Layout {
    /// The layout of a little-endian ELF relocatable object. Only that: a linked file has program
    /// headers whose offsets would move.
    fn read(data: &[u8]) -> Option<Self> {
        if data.len() < 64 || &data[..4] != b"\x7fELF" || data[5] != 1 {
            return None;
        }
        let wide = match data[4] {
            1 => false,
            2 => true,
            _ => return None,
        };
        let u16_at = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?));
        let u32_at = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
        let word = |at: usize| {
            if wide {
                Some(u64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
            } else {
                u32_at(at).map(u64::from)
            }
        };
        if u16_at(16)? != 1 {
            return None;
        }
        let (shoff_at, ehsize) = if wide { (0x28, 64) } else { (0x20, 52) };
        let shoff = usize::try_from(word(shoff_at)?).ok()?;
        let (shentsize, shnum, shstrndx) = if wide {
            (u16_at(0x3a)?, u16_at(0x3c)?, u16_at(0x3e)?)
        } else {
            (u16_at(0x2e)?, u16_at(0x30)?, u16_at(0x32)?)
        };
        let (entsize, count) = (usize::from(shentsize), usize::from(shnum));
        if count == 0 || shoff.checked_add(entsize * count)? > data.len() {
            return None;
        }
        // The offsets of sh_flags, sh_offset, sh_size, sh_info and sh_addralign.
        let fields = if wide {
            [8, 24, 32, 44, 48]
        } else {
            [8, 16, 20, 28, 32]
        };
        let mut headers = Vec::with_capacity(count);
        for i in 0..count {
            let at = shoff + i * entsize;
            headers.push(Header {
                at,
                name: u32_at(at)?,
                kind: u32_at(at + 4)?,
                flags: word(at + fields[0])?,
                offset: word(at + fields[1])?,
                size: word(at + fields[2])?,
                info: u32_at(at + fields[3])?,
                align: word(at + fields[4])?,
            });
        }
        Some(Self {
            wide,
            shoff_at,
            ehsize,
            shoff,
            entsize,
            headers,
            shstrndx: usize::from(shstrndx),
        })
    }

    /// Which sections go: the debug sections and the relocations of them.
    fn debug(&self, data: &[u8], btf: bool) -> Option<Vec<bool>> {
        let names = self.headers.get(self.shstrndx)?;
        let names_at = usize::try_from(names.offset).ok()?;
        let names = data.get(names_at..names_at.checked_add(usize::try_from(names.size).ok()?)?)?;
        let name_of = |h: &Header| {
            let from = usize::try_from(h.name).ok()?;
            let rest = names.get(from..)?;
            Some(&rest[..rest.iter().position(|b| *b == 0)?])
        };
        let debug: Vec<bool> = self
            .headers
            .iter()
            .map(|h| {
                h.flags & object::elf::SHF_ALLOC.0 == 0
                    && name_of(h).is_some_and(|n| {
                        n.starts_with(b".debug_") || (btf && (n == b".BTF" || n == b".BTF.base"))
                    })
            })
            .collect();
        let relocates = |h: &Header| {
            (h.kind == object::elf::SHT_REL.0 || h.kind == object::elf::SHT_RELA.0)
                && usize::try_from(h.info)
                    .ok()
                    .and_then(|t| debug.get(t))
                    .copied()
                    .unwrap_or(false)
        };
        Some(
            self.headers
                .iter()
                .enumerate()
                .map(|(i, h)| debug[i] || relocates(h))
                .collect(),
        )
    }

    /// Write a value of the file's word size at `at`.
    fn put(&self, out: &mut [u8], at: usize, value: u64) -> Option<()> {
        if self.wide {
            out[at..at + 8].copy_from_slice(&value.to_le_bytes());
        } else {
            out[at..at + 4].copy_from_slice(&u32::try_from(value).ok()?.to_le_bytes());
        }
        Some(())
    }
}

fn stripped(data: &[u8], btf: bool) -> Option<Vec<u8>> {
    if data.ends_with(b"~Module signature appended~\n") {
        return None;
    }
    let layout = Layout::read(data)?;
    let dropped = layout.debug(data, btf)?;
    if !dropped.contains(&true) {
        return None;
    }
    let headers = &layout.headers;
    // The kept contents in the order they were in, each at its own alignment.
    let mut order: Vec<usize> = (1..headers.len())
        .filter(|&i| {
            !dropped[i] && headers[i].kind != object::elf::SHT_NOBITS.0 && headers[i].size > 0
        })
        .collect();
    order.sort_by_key(|&i| headers[i].offset);
    let mut out = data.get(..layout.ehsize)?.to_vec();
    let mut placed = vec![None; headers.len()];
    for i in order {
        let h = &headers[i];
        let align = usize::try_from(h.align.max(1)).ok()?;
        out.resize(out.len().div_ceil(align) * align, 0);
        placed[i] = Some(out.len() as u64);
        let from = usize::try_from(h.offset).ok()?;
        out.extend_from_slice(data.get(from..from.checked_add(usize::try_from(h.size).ok()?)?)?);
    }
    let end = out.len() as u64;
    out.resize(out.len().div_ceil(8) * 8, 0);
    let table = out.len();
    out.extend_from_slice(data.get(layout.shoff..layout.shoff + layout.entsize * headers.len())?);
    let (offset_of, size_of) = if layout.wide { (24, 32) } else { (16, 20) };
    for (i, h) in headers.iter().enumerate().skip(1) {
        let at = table + (h.at - layout.shoff);
        if dropped[i] {
            out[at + 4..at + 8].copy_from_slice(&object::elf::SHT_NOBITS.0.to_le_bytes());
            layout.put(&mut out, at + size_of, 0)?;
        }
        layout.put(&mut out, at + offset_of, placed[i].unwrap_or(end))?;
    }
    layout.put(&mut out, layout.shoff_at, table as u64)?;
    Some(out)
}

/// What is wrong with a module, empty when nothing is.
#[must_use]
pub fn problems(m: &Module, release: &str, symvers: &BTreeMap<String, Export>) -> Vec<String> {
    let mut out = Vec::new();
    for (r_type, n) in m.relocs.iter().filter(|_| m.audited) {
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
        "{} modules, {bad} with problems, {skipped} of a machine whose relocations are not audited yet.\n",
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

    /// A module of `arch` with code, `.modinfo`, and DWARF that is relocated against the code.
    fn with_debug(arch: object::Architecture) -> Vec<u8> {
        use object::write::{Object, Relocation, StandardSection, Symbol, SymbolSection};
        use object::{SectionKind, SymbolFlags, SymbolKind, SymbolScope};
        let mut obj = Object::new(object::BinaryFormat::Elf, arch, object::Endianness::Little);
        let text = obj.section_id(StandardSection::Text);
        obj.append_section_data(text, &[0xc3; 32], 16);
        let f = obj.add_symbol(Symbol {
            name: b"init_module".to_vec(),
            value: 0,
            size: 32,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
        let info = obj.add_section(Vec::new(), b".modinfo".to_vec(), SectionKind::Data);
        obj.append_section_data(info, b"license=GPL\0vermagic=7.2.8 SMP\0", 1);
        let btf = obj.add_section(Vec::new(), b".BTF".to_vec(), SectionKind::Metadata);
        obj.append_section_data(btf, &[9; 2048], 4);
        for name in [".debug_info", ".debug_str"] {
            let id = obj.add_section(Vec::new(), name.as_bytes().to_vec(), SectionKind::Debug);
            obj.append_section_data(id, &[7; 4096], 1);
            let flags = object::RelocationFlags::Elf {
                r_type: if arch == object::Architecture::I386 {
                    object::elf::R_386_32
                } else {
                    object::elf::R_X86_64_64
                },
            };
            let reloc = Relocation {
                offset: 8,
                symbol: f,
                addend: 0,
                flags,
            };
            obj.add_relocation(id, reloc).unwrap();
        }
        obj.write().unwrap()
    }

    #[test]
    fn a_module_for_the_initramfs_loses_its_debug_sections_and_nothing_else() {
        use object::{Object, ObjectSection, ObjectSymbol};
        for arch in [object::Architecture::X86_64, object::Architecture::I386] {
            let full = with_debug(arch);
            let small = without_debug(&full);
            assert!(
                small.len() + 8000 < full.len(),
                "{arch:?}: {} of {}",
                small.len(),
                full.len()
            );
            let (a, b) = (
                object::File::parse(&*full).unwrap(),
                object::File::parse(&*small).unwrap(),
            );
            assert_eq!(a.sections().count(), b.sections().count(), "{arch:?}");
            for (x, y) in a.sections().zip(b.sections()) {
                let name = x.name().unwrap();
                assert_eq!(name, y.name().unwrap(), "{arch:?}");
                if name.starts_with(".debug_") || name.starts_with(".rel") {
                    assert_eq!(y.size(), 0, "{arch:?}: {name}");
                } else {
                    assert_eq!(x.data().unwrap(), y.data().unwrap(), "{arch:?}: {name}");
                }
            }
            let names = |f: &object::File| {
                f.symbols()
                    .map(|s| (s.name().unwrap().to_string(), s.section_index()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(names(&a), names(&b), "{arch:?}");
            assert_eq!(read(&small).unwrap().vermagic, "7.2.8 SMP", "{arch:?}");
            assert_eq!(
                without_debug(&small),
                small,
                "{arch:?}: nothing is left to take"
            );
        }
        let mut signed = with_debug(object::Architecture::X86_64);
        signed.extend(b"~Module signature appended~\n");
        assert_eq!(without_debug(&signed), signed);
        assert_eq!(without_debug(b"not a module"), b"not a module");
    }

    #[test]
    fn a_module_for_another_kernel_loses_its_btf_as_well() {
        use object::{Object, ObjectSection};
        let full = with_debug(object::Architecture::X86_64);
        let section = |data: &[u8], name: &str| {
            let file = object::File::parse(data).unwrap();
            file.section_by_name(name).unwrap().size()
        };
        assert_eq!(section(&without_debug(&full), ".BTF"), 2048);
        let small = without_debug_or_btf(&full);
        assert_eq!(section(&small, ".BTF"), 0);
        assert_eq!(section(&small, ".debug_info"), 0);
        assert_eq!(read(&small).unwrap().vermagic, "7.2.8 SMP");
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

    /// A module of `arch` that imports `printk` with `crc`, with the `unsigned long` of its
    /// `__versions` entry as wide as the machine's.
    fn imports(arch: object::Architecture, crc: u32) -> Vec<u8> {
        use object::SectionKind;
        use object::write::{Object, StandardSection};
        let mut obj = Object::new(object::BinaryFormat::Elf, arch, object::Endianness::Little);
        let text = obj.section_id(StandardSection::Text);
        obj.append_section_data(text, &[0; 4], 4);
        let info = obj.add_section(Vec::new(), b".modinfo".to_vec(), SectionKind::Data);
        obj.append_section_data(info, b"license=GPL\0vermagic=6.12.111 SMP\0", 1);
        let mut entry = if arch.address_size() == Some(object::AddressSize::U64) {
            u64::from(crc).to_le_bytes().to_vec()
        } else {
            crc.to_le_bytes().to_vec()
        };
        entry.extend(b"printk");
        entry.resize(64, 0);
        let versions = obj.add_section(Vec::new(), b"__versions".to_vec(), SectionKind::Data);
        obj.append_section_data(versions, &entry, 8);
        obj.write().unwrap()
    }

    #[test]
    fn the_crcs_of_a_machine_whose_relocations_are_not_audited_are_checked() {
        for arch in [object::Architecture::Aarch64, object::Architecture::I386] {
            let m = read(&imports(arch, 0x1234_5678)).unwrap();
            assert!(!m.audited, "{arch:?}");
            assert_eq!(m.versions["printk"], 0x1234_5678, "{arch:?}");
            assert!(problems(&m, "6.12.111", &symvers()).is_empty(), "{arch:?}");
            let m = read(&imports(arch, 0x0bad_c0de)).unwrap();
            let p = problems(&m, "6.12.111", &symvers());
            assert_eq!(p.len(), 1, "{arch:?}: {p:?}");
            assert!(
                p[0].contains("printk imported with CRC 0x0badc0de"),
                "{arch:?}: {p:?}"
            );
        }
    }
}
