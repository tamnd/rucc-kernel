//! The bring-up delegation of the plan's document 00.
//!
//! While rucc cannot yet build some classes of unit, a run may hand them to another compiler so
//! that the rest of the kernel can be built and booted. Two classes exist:
//!
//! - `m16`: every call with `-m16` or `-m32`, which on x86-64 is the real mode setup code and the
//!   32-bit vDSO, and which needs the i686 back end of K4.
//! - `as`: every call whose input is a `.S` file, for the assembler work of K2.
//!
//! A delegated call is written on the record with its class, and a run with any delegated call is
//! ungraded. This module only decides the class. It is the one place such a decision is made, so
//! that delegation can never happen by accident.

use crate::args::{Invocation, Mode};
use std::path::Path;

/// The classes that exist, for checking a user's `RK_BRINGUP`.
pub const CLASSES: &[&str] = &["m16", "as"];

/// The first enabled class the call falls in, if any.
#[must_use]
pub fn class_of(args: &[String], invocation: &Invocation, enabled: &[String]) -> Option<String> {
    if invocation.mode == Mode::Query {
        return None;
    }
    let on = |class: &str| enabled.iter().any(|e| e == class);
    if on("m16") && args.iter().any(|a| a == "-m16" || a == "-m32") {
        return Some("m16".to_string());
    }
    let assembles = invocation.inputs.iter().any(|input| {
        Path::new(input)
            .extension()
            .is_some_and(|ext| ext == "S" || ext == "s")
    }) || args
        .windows(2)
        .any(|w| w[0] == "-x" && w[1] == "assembler-with-cpp");
    if on("as") && assembles {
        return Some("as".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::read;

    fn class(args: &[&str], files: &[&str], enabled: &[&str]) -> Option<String> {
        let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let files: Vec<std::path::PathBuf> =
            files.iter().map(|f| Path::new("/b").join(f)).collect();
        let inv = read(&args, Path::new("/b"), &|p| files.iter().any(|f| f == p));
        let enabled: Vec<String> = enabled.iter().map(|s| (*s).to_string()).collect();
        class_of(&args, &inv, &enabled)
    }

    #[test]
    fn nothing_is_delegated_unless_asked() {
        assert_eq!(
            class(&["-m16", "-c", "boot/main.c"], &["boot/main.c"], &[]),
            None
        );
    }

    #[test]
    fn m16_takes_the_real_mode_and_32_bit_units() {
        let on = &["m16"];
        assert_eq!(
            class(&["-m16", "-c", "a.c"], &["a.c"], on).as_deref(),
            Some("m16")
        );
        assert_eq!(
            class(&["-m32", "-c", "vdso32/x.c"], &["vdso32/x.c"], on).as_deref(),
            Some("m16")
        );
        assert_eq!(class(&["-m64", "-c", "a.c"], &["a.c"], on), None);
    }

    #[test]
    fn as_takes_preprocessed_assembly_but_not_c() {
        let on = &["as"];
        assert_eq!(
            class(&["-c", "entry_64.S"], &["entry_64.S"], on).as_deref(),
            Some("as")
        );
        assert_eq!(class(&["-c", "fork.c"], &["fork.c"], on), None);
    }

    #[test]
    fn questions_are_never_delegated() {
        assert_eq!(class(&["-m32", "--version"], &[], &["m16"]), None);
    }
}
