//! `rk vec-audit`: no vector or x87 instruction in a unit the kernel builds without them.
//!
//! The kernel saves the FPU and vector registers of a task only around `kernel_fpu_begin()` and
//! `kernel_fpu_end()`, so any other kernel code that touches them corrupts user state without a
//! crash to show for it. kbuild stops GCC from using them with `-mno-sse -mno-mmx -mno-sse2
//! -mno-3dnow -mno-avx -mno-80387` on x86 and `-mgeneral-regs-only` on arm64, and a compiler that
//! takes the flags and emits SSE anyway, for a memcpy or a floating point constant, is the bug
//! this looks for (plan 8.3).
//!
//! Every object whose own `.cmd` command line leaves it without the FPU is decoded, and each
//! instruction that names an `xmm`, `ymm`, `zmm`, `mm`, `st` or mask register, or that is an x87
//! instruction at all, is counted by mnemonic. The kernel's own inline asm puts some of these in
//! no-FPU units on purpose, in the RAID and crypto code and in the FPU code itself, so given a
//! reference only the counts above the reference's count for the same object are findings.
//!
//! Only x86-64 is decoded here. The arm64 FP and SIMD check comes with the arm64 rows at K6, and
//! an object of any other machine is skipped.

use crate::objects;
use iced_x86::{Decoder, DecoderOptions, Instruction, Register};
use object::{Object as _, ObjectSection as _};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// Flagged instructions by mnemonic, for one object.
pub type Counts = BTreeMap<String, u64>;

/// Flagged instructions of every no-FPU object of a build, by relative path. An object with none
/// is kept with an empty map, so that a comparison knows it was looked at.
pub type Inventory = BTreeMap<String, Counts>;

/// Whether a compile command leaves the unit without the FPU and vector registers. The last
/// word that decides wins, the way the compiler reads them.
#[must_use]
pub fn no_fpu(words: &[String]) -> bool {
    let mut off = false;
    for w in words {
        match w.as_str() {
            "-mno-sse" | "-mno-80387" | "-mgeneral-regs-only" | "-msoft-float" => off = true,
            "-msse" | "-msse2" | "-mavx" | "-mavx2" | "-mhard-float" | "-m80387" => off = false,
            _ => {}
        }
    }
    off
}

fn is_vector(r: Register) -> bool {
    r.is_xmm() || r.is_ymm() || r.is_zmm() || r.is_mm() || r.is_st() || r.is_k() || r.is_tmm()
}

/// Whether an instruction uses the FPU or a vector register.
#[must_use]
pub fn flagged(instr: &Instruction) -> bool {
    (0..instr.op_count()).any(|i| is_vector(instr.op_register(i)))
        || is_vector(instr.memory_index())
        || instr
            .cpuid_features()
            .iter()
            .any(|f| format!("{f:?}").contains("FPU"))
}

/// Count the flagged instructions in x86-64 code.
pub fn count_code(code: &[u8], counts: &mut Counts) {
    let mut decoder = Decoder::new(64, code, DecoderOptions::NONE);
    let mut instr = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut instr);
        if !instr.is_invalid() && flagged(&instr) {
            *counts
                .entry(format!("{:?}", instr.mnemonic()).to_lowercase())
                .or_default() += 1;
        }
    }
}

/// The flagged instructions of one object, or `None` when it is not x86-64.
pub fn read(data: &[u8]) -> Result<Option<Counts>, String> {
    let file = object::File::parse(data).map_err(|e| e.to_string())?;
    if file.architecture() != object::Architecture::X86_64 {
        return Ok(None);
    }
    let mut counts = Counts::new();
    for section in file.sections() {
        if section.kind() == object::SectionKind::Text {
            count_code(section.data().map_err(|e| e.to_string())?, &mut counts);
        }
    }
    Ok(Some(counts))
}

/// Every no-FPU x86-64 object of a build directory, by its own `.cmd` files.
pub fn scan(out: &Path) -> Result<Inventory, String> {
    let commands = crate::flags::load(out)?;
    let found = objects::find(out)?;
    let mut inventory = Inventory::new();
    for (object, words) in &commands {
        if !no_fpu(words) {
            continue;
        }
        let Some(path) = found.get(object) else {
            continue;
        };
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        if let Ok(Some(counts)) = read(&data) {
            inventory.insert(object.clone(), counts);
        }
    }
    Ok(inventory)
}

/// The findings: for each object, the mnemonics above the reference's count, or every flagged
/// mnemonic without a reference. An object the reference did not audit is judged alone.
#[must_use]
pub fn findings(build: &Inventory, reference: Option<&Inventory>) -> Inventory {
    let empty = Counts::new();
    let mut out = Inventory::new();
    for (object, counts) in build {
        let base = reference.and_then(|r| r.get(object)).unwrap_or(&empty);
        let excess: Counts = counts
            .iter()
            .filter_map(|(m, n)| {
                let over = n.saturating_sub(base.get(m).copied().unwrap_or(0));
                (over > 0).then(|| (m.clone(), over))
            })
            .collect();
        if !excess.is_empty() {
            out.insert(object.clone(), excess);
        }
    }
    out
}

/// The findings as markdown.
#[must_use]
pub fn report(audited: usize, findings: &Inventory, against_reference: bool) -> String {
    let mut s = String::from("### Vector and x87 instructions in no-FPU units\n\n");
    let _ = writeln!(
        s,
        "{audited} no-FPU objects decoded, {} with {}.\n",
        findings.len(),
        if against_reference {
            "more vector or x87 instructions than the reference"
        } else {
            "vector or x87 instructions"
        }
    );
    if !findings.is_empty() {
        s.push_str("| object | instructions |\n|---|---|\n");
        for (object, counts) in findings.iter().take(100) {
            let list: Vec<String> = counts.iter().map(|(m, n)| format!("{m} x{n}")).collect();
            let _ = writeln!(s, "| {object} | {} |", list.join(", "));
        }
        if findings.len() > 100 {
            let _ = writeln!(s, "\nand {} more.", findings.len() - 100);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::fixture::Builder;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_last_flag_decides() {
        assert!(no_fpu(&words("gcc -mno-sse -mno-mmx -mno-80387 -c a.c")));
        assert!(!no_fpu(&words("gcc -mno-sse -msse2 -c a.c")));
        assert!(no_fpu(&words("gcc -mgeneral-regs-only -c a.c")));
        assert!(!no_fpu(&words("gcc -O2 -c a.c")));
    }

    #[test]
    fn sse_and_x87_are_counted_and_plain_code_is_not() {
        let mut counts = Counts::new();
        // mov rbx, rax; movaps xmm0, xmm1; fldz; fld qword [rax]; lfence; ret
        count_code(
            &[
                0x48, 0x89, 0xc3, 0x0f, 0x28, 0xc1, 0xd9, 0xee, 0xdd, 0x00, 0x0f, 0xae, 0xe8, 0xc3,
            ],
            &mut counts,
        );
        let got: Vec<(&str, u64)> = counts.iter().map(|(m, n)| (m.as_str(), *n)).collect();
        assert_eq!(got, [("fld", 1), ("fldz", 1), ("movaps", 1)]);
    }

    #[test]
    fn only_counts_above_the_reference_are_findings() {
        let mut b = Builder::new();
        b.function("memcpy_like", &[0x0f, 0x28, 0xc1, 0x0f, 0x28, 0xc1, 0xc3]);
        let rucc = read(&b.bytes()).unwrap().unwrap();
        let mut b = Builder::new();
        b.function("memcpy_like", &[0x0f, 0x28, 0xc1, 0xc3]);
        let gcc = read(&b.bytes()).unwrap().unwrap();
        let build: Inventory = [("a.o".to_string(), rucc)].into();
        let reference: Inventory = [("a.o".to_string(), gcc)].into();
        let f = findings(&build, Some(&reference));
        assert_eq!(f["a.o"]["movaps"], 1);
        assert!(findings(&reference, Some(&reference)).is_empty());
        assert!(report(1, &f, true).contains("| a.o | movaps x1 |"));
    }
}
