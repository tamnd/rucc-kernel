//! Splats in a kernel log, normalized so that two kernels' logs can be compared (11.9).
//!
//! A splat is a warning, an oops or any other report the kernel prints when something went wrong
//! without necessarily stopping it. Each one is known by its first line, and that line is
//! normalized: the printk timestamp is taken off, and so is everything that changes from boot to
//! boot or from compiler to compiler, which is addresses, offsets into functions, PIDs, CPU
//! numbers and the numbers in thread names. Source locations like `kernel/fork.c:123` are kept,
//! since both kernels are built from the same tree.
//!
//! The rucc kernel's set of normalized splats must be a subset of the reference's. A splat only
//! the rucc kernel prints is a failure even when every test passed, and it is how a broken memory
//! model is most likely to show first.

use std::collections::BTreeSet;

/// What the first line of a splat contains.
const MARKERS: &[&str] = &[
    "WARNING:",
    "BUG:",
    "Oops",
    "general protection fault",
    "kernel BUG at",
    "KASAN:",
    "UBSAN:",
    "KFENCE:",
    "list_add corruption",
    "list_del corruption",
    "self-detected stall",
    "detected stalls on CPUs",
    "blocked for more than",
    "soft lockup",
    "hard LOCKUP",
    "possible circular locking dependency",
    "possible recursive locking",
    "inconsistent lock state",
    "suspicious RCU usage",
    "Unable to handle kernel",
    "stack-protector:",
    "stack guard page was hit",
    "Kernel panic",
    "unwind: ",
];

/// Whether a line, without its timestamp, starts a splat.
///
/// A line that starts `traps: ` is the kernel saying it sent a signal to a user program that
/// faulted, as in `traps: fsgsbase_64[417] general protection fault ip:202c10 ...`. That is the
/// kernel working, and the program is either a test whose own result says whether it passed or a
/// program built by the other compiler, so it is not a splat of the kernel's. A fault inside the
/// kernel prints `Oops: general protection fault` and is still one.
#[must_use]
pub fn is_splat(text: &str) -> bool {
    !text.starts_with("traps: ") && MARKERS.iter().any(|m| text.contains(m))
}

/// A word with the parts that change between boots and compilers masked.
fn mask_word(word: &str) -> String {
    if word.contains(".c:") || word.contains(".h:") || word.contains(".S:") {
        return word.to_string();
    }
    if let Some((name, _)) = word.split_once("+0x") {
        return format!("{name}+N");
    }
    let mut out = String::new();
    let mut chars = word.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '0' && chars.peek() == Some(&'x') {
            chars.next();
            while chars.peek().is_some_and(char::is_ascii_hexdigit) {
                chars.next();
            }
            out.push('N');
        } else if c.is_ascii_digit() {
            while chars.peek().is_some_and(char::is_ascii_hexdigit) {
                chars.next();
            }
            out.push('N');
        } else {
            out.push(c);
        }
    }
    // A bare address with no digits in front, like ffffffff81000000, has letters first.
    if out.len() >= 8 && word.len() >= 8 && word.chars().all(|c| c.is_ascii_hexdigit()) {
        return "N".to_string();
    }
    out
}

/// A splat's first line, normalized.
#[must_use]
pub fn normalize(text: &str) -> String {
    text.split_whitespace()
        .map(mask_word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The normalized splats of a console log.
#[must_use]
pub fn splats(console: &str) -> BTreeSet<String> {
    console
        .lines()
        .map(|line| crate::boot::strip_timestamp(line.trim_end_matches('\r')).trim())
        .filter(|text| !text.starts_with("RK-") && is_splat(text))
        .map(normalize)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_splat_is_known_by_its_first_line_without_what_changes() {
        let a = "[    3.141592] WARNING: CPU: 1 PID: 42 at kernel/fork.c:123 copy_process+0x1a2/0x2000\n\
                 [    3.141593] Modules linked in: foo\n\
                 [    4.000000] kworker/u4:1 (61) used greatest stack depth: 13584 bytes left\n";
        let b =
            "[    9.000000] WARNING: CPU: 0 PID: 7 at kernel/fork.c:123 copy_process+0x80/0x1800\n";
        assert_eq!(splats(a), splats(b));
        assert_eq!(
            splats(a).into_iter().collect::<Vec<_>>(),
            ["WARNING: CPU: N PID: N at kernel/fork.c:123 copy_process+N"]
        );
    }

    #[test]
    fn addresses_and_thread_numbers_are_masked() {
        assert_eq!(
            normalize("BUG: unable to handle page fault for address: ffffffff81000000"),
            "BUG: unable to handle page fault for address: N"
        );
        assert_eq!(
            normalize("INFO: task kworker/u4:1:61 blocked for more than 120 seconds."),
            "INFO: task kworker/uN:N:N blocked for more than N seconds."
        );
        assert_eq!(
            normalize("list_add corruption. prev->next should be next (0xdead)"),
            "list_add corruption. prev->next should be next (N)"
        );
    }

    #[test]
    fn ordinary_lines_and_rk_lines_are_not_splats() {
        assert!(
            splats("[ 1.0] Freeing unused kernel image memory: 2048K\nRK-CHECK fork pass\n")
                .is_empty()
        );
    }

    #[test]
    fn a_user_program_that_faults_is_not_a_kernel_splat() {
        let console = "[ 163.048680] traps: fsgsbase_64[417] general protection fault ip:202c10 \
                       sp:7f9aabb32e60 error:402 in fsgsbase_64[1c10,202000+2000]\n\
                       [ 170.000000] Oops: general protection fault, probably for non-canonical \
                       address 0xdead000000000100: 0000 [#1] SMP\n";
        assert_eq!(
            splats(console).into_iter().collect::<Vec<_>>(),
            ["Oops: general protection fault, probably for non-canonical address N: N [#N] SMP"]
        );
    }
}
