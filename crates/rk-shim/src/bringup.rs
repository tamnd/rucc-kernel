//! The bring-up delegation of the plan's document 00.
//!
//! While rucc cannot yet build some classes of unit, a run may hand them to another compiler so
//! that the rest of the kernel can be built and booted. One class is left:
//!
//! - `m16`: every call with `-m16` or `-m32`, which on x86-64 is the real mode setup code and the
//!   32-bit vDSO, and which needs the i686 back end of K4.
//!
//! The `as` class, which sent every `.S` file away, is retired. K2 made rucc assemble the kernel,
//! and its exit criterion allows bring-up for `-m16` and `-m32` only, so asking for `as` is an
//! error rather than a quiet way around the assembler.
//!
//! A delegated call is written on the record with its class, and a run with any delegated call is
//! ungraded. This module only decides the class. It is the one place such a decision is made, so
//! that delegation can never happen by accident.

use crate::args::{Invocation, Mode};

/// The classes that exist, for checking a user's `RK_BRINGUP`.
pub const CLASSES: &[&str] = &["m16"];

/// Classes that existed once and are refused now, with the reason.
const RETIRED: &[(&str, &str)] = &[(
    "as",
    "rucc assembles every .S unit since K2, and only -m16 and -m32 may be delegated",
)];

/// Checks a list of classes before any call is made, so that a typo or a retired class fails
/// the whole run up front instead of building everything with rucc and grading it.
///
/// # Errors
///
/// Names the first class that is retired or unknown.
pub fn check(enabled: &[String]) -> Result<(), String> {
    for class in enabled {
        if let Some((_, why)) = RETIRED.iter().find(|(name, _)| name == class) {
            return Err(format!("bring-up class {class} is retired: {why}"));
        }
        if !CLASSES.contains(&class.as_str()) {
            return Err(format!(
                "bring-up class {class} is unknown, the classes are {}",
                CLASSES.join(", ")
            ));
        }
    }
    Ok(())
}

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
    None
}

/// The arguments a call hands to its compiler.
///
/// rk puts the era's persona in `CC` as `-fgnuc-version=`, which only rucc knows. GCC refuses
/// it, so a delegated call leaves it out. Every other argument goes through as kbuild wrote it.
#[must_use]
pub fn passed_on(args: &[String], delegated: bool) -> Vec<String> {
    args.iter()
        .filter(|a| !(delegated && a.starts_with("-fgnuc-version=")))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::read;
    use std::path::Path;

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
    fn assembly_stays_with_the_compiler_under_test() {
        let on = &["m16", "as"];
        assert_eq!(class(&["-c", "entry_64.S"], &["entry_64.S"], on), None);
        assert_eq!(
            class(
                &["-x", "assembler-with-cpp", "-c", "head_64.S"],
                &["head_64.S"],
                on
            ),
            None
        );
    }

    fn list(classes: &[&str]) -> Vec<String> {
        classes.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn m16_is_the_only_class_accepted() {
        assert_eq!(check(&list(&["m16"])), Ok(()));
        assert_eq!(check(&[]), Ok(()));
    }

    #[test]
    fn the_retired_as_class_is_refused_with_its_reason() {
        let err = check(&list(&["m16", "as"])).unwrap_err();
        assert!(err.contains("as is retired"), "{err}");
        assert!(err.contains("-m16 and -m32"), "{err}");
    }

    #[test]
    fn an_unknown_class_is_refused() {
        let err = check(&list(&["m61"])).unwrap_err();
        assert_eq!(err, "bring-up class m61 is unknown, the classes are m16");
    }

    #[test]
    fn questions_are_never_delegated() {
        assert_eq!(class(&["-m32", "--version"], &[], &["m16"]), None);
    }

    #[test]
    fn the_persona_is_not_passed_on_to_the_other_compiler() {
        let args: Vec<String> = ["-fgnuc-version=14.2.0", "-m16", "-c", "-o", "a.o", "a.S"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(passed_on(&args, true), ["-m16", "-c", "-o", "a.o", "a.S"]);
        assert_eq!(passed_on(&args, false), args, "rucc keeps it");
    }
}
