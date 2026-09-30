//! The object files of a build directory, and the functions in them.
//!
//! The audits of K2 read the ELF objects a build left behind: every `*.o` and `*.ko` under the
//! output directory, by path relative to it. `vmlinux.o` is left out, since it is every other
//! object linked together and would count each table twice, and so are kbuild's `.tmp_` files
//! and the shim's `rk-bin`. Symbolic links are not followed, so the `source` link back to the
//! tree is not walked.
//!
//! An audit keeps what it found as JSON, so that a build job can reduce three thousand objects to
//! one small file and a later job can compare two of them without the objects.

use object::{Object as _, ObjectSection as _, ObjectSymbol as _, RelocationTarget, SymbolKind};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Every object under a build directory, by path relative to it.
pub fn find(out: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut found = BTreeMap::new();
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
            } else if kind.is_file()
                && (extension(&name) == Some("o") || is_module(&name))
                && !name.starts_with(".tmp_")
                && name != "vmlinux.o"
            {
                let path = entry.path();
                let relative = path
                    .strip_prefix(out)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                found.insert(relative, path);
            }
        }
    }
    Ok(found)
}

fn extension(name: &str) -> Option<&str> {
    Path::new(name).extension().and_then(|e| e.to_str())
}

/// Whether a path names a kernel module.
#[must_use]
pub fn is_module(name: &str) -> bool {
    extension(name) == Some("ko")
}

/// An audit's findings: read from a JSON file when `path` is one, or else made by `scan` from
/// the build directory it names.
pub fn load_or_scan<T: DeserializeOwned>(
    path: &Path,
    scan: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    if path.is_file() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("reading {}: {e}", path.display()))
    } else if path.is_dir() {
        scan(path)
    } else {
        Err(format!(
            "{} is neither a build nor a saved audit",
            path.display()
        ))
    }
}

/// Write an audit's findings as JSON.
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// The functions of an object by section index, sorted by address, for finding the function a
/// relocation points into.
pub struct Functions {
    by_section: BTreeMap<usize, Vec<(u64, u64, String)>>,
}

impl Functions {
    /// The function symbols of an object.
    #[must_use]
    pub fn of(file: &object::File<'_>) -> Self {
        let mut by_section: BTreeMap<usize, Vec<(u64, u64, String)>> = BTreeMap::new();
        for symbol in file.symbols() {
            if symbol.kind() != SymbolKind::Text {
                continue;
            }
            let (Some(section), Ok(name)) = (symbol.section_index(), symbol.name()) else {
                continue;
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

    /// The function covering an offset in a section. A function with no size covers only its
    /// first byte.
    #[must_use]
    pub fn at(&self, section: usize, offset: u64) -> Option<&str> {
        let list = self.by_section.get(&section)?;
        let i = list.partition_point(|(start, _, _)| *start <= offset);
        let (start, size, name) = list.get(i.checked_sub(1)?)?;
        (offset < start + (*size).max(1)).then_some(name.as_str())
    }

    /// The function a relocation points at: a function symbol itself, or the function covering
    /// the addend when the symbol is a section.
    #[must_use]
    pub fn target(&self, file: &object::File<'_>, reloc: &object::Relocation) -> Option<String> {
        let RelocationTarget::Symbol(index) = reloc.target() else {
            return None;
        };
        let symbol = file.symbol_by_index(index).ok()?;
        let section = symbol.section_index()?;
        if !file
            .section_by_index(section)
            .is_ok_and(|s| s.kind() == object::SectionKind::Text)
        {
            return None;
        }
        let offset = symbol
            .address()
            .checked_add_signed(reloc.addend())
            .unwrap_or_default();
        if symbol.kind() == SymbolKind::Text && offset == symbol.address() {
            return symbol.name().ok().map(str::to_string);
        }
        self.at(section.0, offset).map(str::to_string)
    }
}

/// Small ELF objects for the tests, made with `object`'s writer.
#[cfg(test)]
pub mod fixture {
    use object::write::{Object, Relocation, StandardSection, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };

    /// An x86-64 relocatable object being built.
    pub struct Builder {
        /// The object.
        pub obj: Object<'static>,
    }

    impl Builder {
        /// An empty object.
        pub fn new() -> Self {
            Self {
                obj: Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little),
            }
        }

        /// A global function with its code in `.text`.
        pub fn function(&mut self, name: &str, code: &[u8]) -> object::write::SymbolId {
            let text = self.obj.section_id(StandardSection::Text);
            let offset = self.obj.append_section_data(text, code, 16);
            self.obj.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: offset,
                size: code.len() as u64,
                kind: SymbolKind::Text,
                scope: SymbolScope::Linkage,
                weak: false,
                section: SymbolSection::Section(text),
                flags: SymbolFlags::None,
            })
        }

        /// A data section with the given bytes.
        pub fn section(&mut self, name: &str, data: &[u8]) -> object::write::SectionId {
            let id = self
                .obj
                .add_section(Vec::new(), name.as_bytes().to_vec(), SectionKind::Data);
            self.obj.append_section_data(id, data, 1);
            id
        }

        /// A relocation of type `r_type` in `section` at `offset` against `symbol`.
        pub fn reloc(
            &mut self,
            section: object::write::SectionId,
            offset: u64,
            symbol: object::write::SymbolId,
            r_type: object::elf::RelocationType,
        ) {
            self.obj
                .add_relocation(
                    section,
                    Relocation {
                        offset,
                        symbol,
                        addend: 0,
                        flags: RelocationFlags::Elf { r_type },
                    },
                )
                .unwrap();
        }

        /// The bytes of the object.
        pub fn bytes(&self) -> Vec<u8> {
            self.obj.write().unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relocation_finds_its_function() {
        let mut b = fixture::Builder::new();
        b.function("first", &[0x90; 8]);
        let second = b.function("second", &[0xc3]);
        let table = b.section("__mcount_loc", &[0; 8]);
        b.reloc(table, 0, second, object::elf::R_X86_64_64);
        let bytes = b.bytes();
        let file = object::File::parse(&*bytes).unwrap();
        let functions = Functions::of(&file);
        let section = file.section_by_name("__mcount_loc").unwrap();
        let (_, reloc) = section.relocations().next().unwrap();
        assert_eq!(functions.target(&file, &reloc).as_deref(), Some("second"));
        let text = file.section_by_name(".text").unwrap().index().0;
        assert_eq!(functions.at(text, 3), Some("first"));
        assert_eq!(functions.at(text, 100), None);
    }

    #[test]
    fn the_walk_skips_vmlinux_o_and_the_shim() {
        let dir = std::env::temp_dir().join(format!("rk-objects-{}", std::process::id()));
        for f in [
            "a/b.o",
            "vmlinux.o",
            "rk-bin/x.o",
            "m.ko",
            "a/.tmp_c.o",
            "a/b.c",
        ] {
            let path = dir.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        let found = find(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found.keys().collect::<Vec<_>>(), ["a/b.o", "m.ko"]);
    }
}
