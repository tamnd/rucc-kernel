//! Splats in a kernel log, normalized so that two kernels' logs can be compared (11.9).
//!
//! A splat is a warning, an oops or any other report the kernel prints when something went wrong
//! without necessarily stopping it. Each one is known by its first line, and that line is
//! normalized: the printk timestamp is taken off, and so is everything that changes from boot to
//! boot or from compiler to compiler, which is addresses, offsets into functions, PIDs, CPU
//! numbers and the numbers in thread names. Source locations like `kernel/fork.c:123` are kept,
//! since both kernels are built from the same tree. What comes after a source location is
//! dropped. The function there is the function the code was inlined into: a `WARN_ON` in a static
//! helper that GCC inlines and rucc does not names a different function at the same line. In
//! newer kernels, like 7.2, a warning also names the CPU and the task after the function, as in
//! `WARNING: kernel/time/hrtimer.c:1071 at hrtimer_forward+0x16a/0x250, CPU#1: swapper/1/0`, and
//! the task is whatever ran when the timer or the test hit the line.
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
///
/// A line that starts `# ` is a kselftest program's own output, which the runner prefixes that way,
/// and a test that prints `BUG:` or `WARNING:` about what it checked is not the kernel's splat
/// either.
#[must_use]
pub fn is_splat(text: &str) -> bool {
    !text.starts_with("traps: ")
        && !text.starts_with("# ")
        && MARKERS.iter().any(|m| text.contains(m))
}

/// Whether a word is a source location, like `kernel/fork.c:123`.
fn is_location(word: &str) -> bool {
    word.contains(".c:") || word.contains(".h:") || word.contains(".S:")
}

/// A word with the parts that change between boots and compilers masked.
fn mask_word(word: &str) -> String {
    if is_location(word) {
        return word.to_string();
    }
    if let Some((name, _)) = word.split_once("+0x") {
        // GCC can put part of a function in a clone, like `f.cold`, `f.isra.0` or
        // `f.constprop.0`, and rucc does not. The splat is the same, so the suffix is removed.
        let name = name.split_once('.').map_or(name, |(base, _)| base);
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
///
/// A soft lockup names in brackets the task that ran when the watchdog fired, as in `watchdog: BUG:
/// soft lockup - CPU#0 stuck for 26s! [swapper/0:0]`. While the x86 kselftests flood the serial
/// console, that is either the idle task or `ksoftirqd`, and the CPU is stuck in the same interrupt
/// for both. So the task is dropped from that line.
#[must_use]
pub fn normalize(text: &str) -> String {
    let lockup = text.contains("soft lockup");
    let mut words = Vec::new();
    for word in text.split_whitespace() {
        if lockup && word.starts_with('[') {
            break;
        }
        words.push(mask_word(word));
        if is_location(word) {
            break;
        }
    }
    words.join(" ")
}

/// Whether two normalized splats are the same splat, with the function that one compiler inlined.
///
/// KASAN names the function that did the bad access, as in `BUG: KASAN: slab-out-of-bounds in
/// krealloc_more_oob_helper+N`. When rucc inlines the helper into `krealloc_more_oob` and GCC does
/// not, the two kernels name different functions in the same report. The two names are the same
/// function when one is the other with a suffix that starts with `_`, which is how the kernel names
/// its helpers. All other words must be equal.
#[must_use]
pub fn same(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
    if a.len() != b.len() {
        return false;
    }
    let mut differ = a.iter().zip(&b).filter(|(x, y)| x != y);
    match (differ.next(), differ.next()) {
        (Some((x, y)), None) => helper_of(x, y) || helper_of(y, x),
        _ => false,
    }
}

/// Whether `helper` is `caller` with a suffix, as `f_helper+N` is to `f+N`.
fn helper_of(helper: &str, caller: &str) -> bool {
    let (Some(helper), Some(caller)) = (helper.strip_suffix("+N"), caller.strip_suffix("+N"))
    else {
        return false;
    };
    helper
        .strip_prefix(caller)
        .is_some_and(|rest| rest.len() > 1 && rest.starts_with('_'))
}

/// The normalized splats of a console log.
///
/// A KASAN report from a KUnit case is known by the case and not by the function. The function
/// is the one that did the bad access after inlining. In the KASAN suite, `krealloc_large_more_oob`
/// only calls `krealloc_more_oob_helper`. GCC makes the call a tail call, so the report names the
/// helper and the case is not in the trace. rucc inlines the helper, so the report names the case.
/// A report goes to the case whose KTAP result line, `ok N name` or `not ok N name`, comes next
/// after a `# Subtest:` line.
#[must_use]
pub fn splats(console: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut in_suite = false;
    let mut pending = Vec::new();
    for line in console.lines() {
        let text = crate::boot::strip_timestamp(line.trim_end_matches('\r')).trim();
        if text.starts_with("# Subtest:") {
            in_suite = true;
            continue;
        }
        if let Some(case) = case_of(text) {
            for kind in pending.drain(..) {
                found.insert(format!("{kind} in case {case}"));
            }
            continue;
        }
        if text.starts_with("RK-") || !is_splat(text) {
            continue;
        }
        let splat = normalize(text);
        match splat
            .strip_prefix("BUG: KASAN: ")
            .and_then(|rest| rest.split_once(" in "))
        {
            Some((kind, _)) if in_suite => pending.push(format!("BUG: KASAN: {kind}")),
            _ => {
                found.insert(splat);
            }
        }
    }
    found.extend(
        pending
            .into_iter()
            .map(|kind| format!("{kind} in a case with no result")),
    );
    found
}

/// The KUnit case of a KTAP result line, as `kmalloc_oob_right` in `ok 1 kmalloc_oob_right`.
fn case_of(text: &str) -> Option<&str> {
    let rest = text
        .strip_prefix("not ok ")
        .or_else(|| text.strip_prefix("ok "))?;
    let (number, rest) = rest.split_once(' ')?;
    if !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    rest.split_whitespace().next()
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
            ["WARNING: CPU: N PID: N at kernel/fork.c:123"]
        );
    }

    #[test]
    fn the_function_a_warning_was_inlined_into_does_not_count() {
        let gcc = "WARNING: CPU: 1 PID: 1918 at drivers/gpu/drm/drm_rect.c:137 \
                   drm_rect_calc_hscale+0x49/0x60";
        let rucc = "WARNING: CPU: 1 PID: 1918 at drivers/gpu/drm/drm_rect.c:137 \
                    drm_calc_scale+0x49/0x60";
        assert_eq!(normalize(gcc), normalize(rucc));
        assert_eq!(
            normalize(gcc),
            "WARNING: CPU: N PID: N at drivers/gpu/drm/drm_rect.c:137"
        );
    }

    #[test]
    fn the_function_and_the_task_after_a_location_do_not_count() {
        let gcc = "WARNING: kernel/time/hrtimer.c:1071 at hrtimer_forward+0x1a0/0x270, CPU#0: \
                   kunit_try_catch/784";
        let rucc = "WARNING: kernel/time/hrtimer.c:1071 at hrtimer_forward+0x16a/0x250, CPU#0: \
                    swapper/0/0";
        assert_eq!(normalize(gcc), normalize(rucc));
        assert_eq!(normalize(gcc), "WARNING: kernel/time/hrtimer.c:1071");
    }

    #[test]
    fn a_clone_of_the_function_counts_as_the_function() {
        let gcc = "BUG: KFENCE: use-after-free read in \
                   test_memcache_typesafe_by_rcu.cold+0xd6/0x1f4 [kfence_test]";
        let rucc = "BUG: KFENCE: use-after-free read in \
                    test_memcache_typesafe_by_rcu+0x131/0x310 [kfence_test]";
        assert_eq!(normalize(gcc), normalize(rucc));
        assert_eq!(normalize("at f.isra.0+0x10/0x20"), "at f+N");
    }

    #[test]
    fn a_report_in_an_inlined_helper_is_the_same_report() {
        let gcc =
            normalize("BUG: KASAN: slab-out-of-bounds in krealloc_more_oob_helper+0x5a1/0x720");
        let rucc = normalize("BUG: KASAN: slab-out-of-bounds in krealloc_more_oob+0x3f2/0x9c0");
        assert!(same(&gcc, &rucc));
        assert!(same(&rucc, &gcc));
        assert!(!same(
            &rucc,
            "BUG: KASAN: use-after-free in krealloc_more_oob_helper+N"
        ));
        assert!(!same(
            &rucc,
            "BUG: KASAN: slab-out-of-bounds in krealloc_more_oobx+N"
        ));
        assert!(!same(
            &rucc,
            "BUG: KASAN: slab-out-of-bounds in kmalloc_oob_right+N"
        ));
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
    fn a_kasan_report_in_a_kunit_case_is_known_by_the_case() {
        let gcc = "[ 1.0] BUG: KASAN: slab-out-of-bounds in krealloc_more_oob_helper+0x5a1/0x720\n\
                   [ 2.0]     ok 13 krealloc_large_more_oob\n";
        let rucc = "[ 1.0]     # Subtest: kasan\n\
                    [ 1.1] BUG: KASAN: slab-out-of-bounds in krealloc_large_more_oob+0x5f5/0x8b0\n\
                    [ 2.0]     ok 13 krealloc_large_more_oob\n\
                    [ 3.0] BUG: KASAN: use-after-free in f+0x10/0x20\n";
        assert_eq!(
            splats(rucc).into_iter().collect::<Vec<_>>(),
            [
                "BUG: KASAN: slab-out-of-bounds in case krealloc_large_more_oob",
                "BUG: KASAN: use-after-free in a case with no result",
            ]
        );
        let gcc = format!("[ 0.5]     # Subtest: kasan\n{gcc}");
        assert!(splats(rucc).contains(splats(&gcc).first().unwrap()));
        assert_eq!(
            splats(gcc.trim_start_matches("[ 0.5]     # Subtest: kasan\n"))
                .into_iter()
                .collect::<Vec<_>>(),
            ["BUG: KASAN: slab-out-of-bounds in krealloc_more_oob_helper+N"]
        );
    }

    #[test]
    fn the_task_of_a_soft_lockup_does_not_count() {
        let gcc = "watchdog: BUG: soft lockup - CPU#0 stuck for 26s! [swapper/0:0]";
        let rucc = "watchdog: BUG: soft lockup - CPU#0 stuck for 24s! [ksoftirqd/0:15]";
        assert_eq!(normalize(gcc), normalize(rucc));
        assert_eq!(
            normalize(gcc),
            "watchdog: BUG: soft lockup - CPU#N stuck for Ns!"
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
    fn a_user_program_that_faults_or_prints_a_marker_is_not_a_kernel_splat() {
        let console = "[ 163.048680] traps: fsgsbase_64[417] general protection fault ip:202c10 \
                       sp:7f9aabb32e60 error:402 in fsgsbase_64[1c10,202000+2000]\n\
                       # [FAIL] BUG: GSBASE was not preserved\n\
                       [ 170.000000] Oops: general protection fault, probably for non-canonical \
                       address 0xdead000000000100: 0000 [#1] SMP\n";
        assert_eq!(
            splats(console).into_iter().collect::<Vec<_>>(),
            ["Oops: general protection fault, probably for non-canonical address N: N [#N] SMP"]
        );
    }
}
