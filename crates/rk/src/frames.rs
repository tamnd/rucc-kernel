//! `rk frames`: the stack frame of every function in two builds, compared.
//!
//! A kernel stack is 16 KB on 64-bit machines, and a rucc frame twice the size of GCC's fails no
//! test until some deep call chain runs off the end of it. kbuild turns `CONFIG_FRAME_WARN` into
//! `-Wframe-larger-than=`, but rucc prints no warnings, so this measures instead (08.8 in the
//! plan). A build made with `rk build --stack-usage` passes `KCFLAGS=-fstack-usage`, which is how
//! the kernel's own `scripts/stackusage` does it, and each compiler then writes a `.su` file next
//! to every object: one line per function with its frame size in bytes.
//!
//! Functions are matched by the object they are in and their name, with GCC's clone suffixes
//! (`.isra.0`, `.constprop.0`, `.part.0`, `.cold`) taken off, so that a function and its clones
//! count as one with the largest frame. A function over `FRAME_WARN` in the other build that the
//! reference keeps under it, or does not have at all, is a finding.
//!
//! The kernel with `CONFIG_DEBUG_STACK_USAGE` also prints the deepest any thread went into its
//! stack, as `used greatest stack depth: N bytes left`. When the build directory has a `boot.log`
//! with such lines, the high water mark is the stack size less the smallest N, and the plan's
//! limit (02.5) is that the other build's is at most 10% above the reference's.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// How far above the reference the other build's high water mark may go, as a ratio.
pub const HIGH_WATER_LIMIT: f64 = 1.10;

/// One function's frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    /// Bytes.
    pub bytes: u64,
    /// GCC's qualifier: `static`, `dynamic` or `dynamic,bounded`.
    pub kind: String,
}

/// What a build says about its stack, which `--save` keeps as JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Frames {
    /// Every function's frame, by `object function`.
    pub functions: BTreeMap<String, Frame>,
    /// `CONFIG_FRAME_WARN`, when the build has a `.config` that sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_warn: Option<u64>,
    /// The deepest any thread went into its stack at run time, in bytes, when a boot log says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_water: Option<u64>,
}

/// A function name with GCC's clone suffixes taken off. C names have no dots, so everything from
/// the first dot on is a suffix.
#[must_use]
pub fn base_name(function: &str) -> &str {
    function.split('.').next().unwrap_or(function)
}

/// Read one `.su` line: `kernel/fork.c:2134:13:copy_process<TAB>392<TAB>static`. Older GCC has
/// no column, and the function is always what follows the last colon.
#[must_use]
pub fn parse_line(line: &str) -> Option<(String, Frame)> {
    let mut fields = line.split('\t');
    let location = fields.next()?;
    let bytes = fields.next()?.trim().parse().ok()?;
    let kind = fields.next().unwrap_or("static").trim().to_string();
    let function = location
        .rsplit_once(':')
        .map_or(location, |(_, f)| f)
        .trim();
    if function.is_empty() {
        return None;
    }
    Some((function.to_string(), Frame { bytes, kind }))
}

/// The object a `.su` file belongs to, from its path relative to the build: `kernel/fork.su` or
/// `kernel/fork.o.su` are both `kernel/fork.o`.
fn object_of(relative: &str) -> String {
    let stem = relative.strip_suffix(".su").unwrap_or(relative);
    let stem = stem.strip_suffix(".o").unwrap_or(stem);
    format!("{stem}.o")
}

/// Add the lines of one `.su` file, keeping the largest frame of a function and its clones.
pub fn add_file(functions: &mut BTreeMap<String, Frame>, relative: &str, text: &str) {
    let object = object_of(relative);
    for (function, frame) in text.lines().filter_map(parse_line) {
        let key = format!("{object} {}", base_name(&function));
        match functions.get(&key) {
            Some(old) if old.bytes >= frame.bytes => {}
            _ => {
                functions.insert(key, frame);
            }
        }
    }
}

/// The stack size of a kernel, from its `.config`: 16 KB on 64-bit and 8 KB on 32-bit, doubled
/// by KASAN. Kernels before 3.15 had 8 KB on x86-64 too, which this does not know about.
fn thread_size(config: &crate::kconfig::Config) -> u64 {
    let on = |s: &str| config.get(s).is_some_and(|v| v == "y");
    let base = if on("64BIT") { 16384 } else { 8192 };
    if on("KASAN") { base * 2 } else { base }
}

/// The fewest bytes left on any stack, from the lines `CONFIG_DEBUG_STACK_USAGE` prints.
#[must_use]
pub fn least_left(log: &str) -> Option<u64> {
    log.lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once("used greatest stack depth: ")?;
            rest.split_whitespace().next()?.parse::<u64>().ok()
        })
        .min()
}

/// Read every `.su` file under a build directory, and its `.config` and `boot.log` if it has them.
pub fn scan(out: &Path) -> Result<Frames, String> {
    let mut frames = Frames::default();
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
                && Path::new(&name).extension().is_some_and(|e| e == "su")
                && !name.starts_with(".tmp_")
            {
                let path = entry.path();
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("reading {}: {e}", path.display()))?;
                let relative = path.strip_prefix(out).unwrap_or(&path).to_string_lossy();
                add_file(&mut frames.functions, &relative, &text);
            }
        }
    }
    if let Ok(config) = crate::kconfig::load(out) {
        frames.frame_warn = config.get("FRAME_WARN").and_then(|v| v.parse().ok());
        if let Some(left) = std::fs::read_to_string(out.join("boot.log"))
            .ok()
            .and_then(|log| least_left(&log))
        {
            frames.high_water = Some(thread_size(&config).saturating_sub(left));
        }
    }
    Ok(frames)
}

/// A function over the limit in the other build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// `object function`.
    pub function: String,
    /// The reference's frame, if it has the function.
    pub reference: Option<u64>,
    /// The other build's frame.
    pub other: Frame,
}

/// The size of a set of frames: how many, the median, the 99th percentile, the largest and the
/// sum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spread {
    /// Functions.
    pub count: usize,
    /// The median frame.
    pub p50: u64,
    /// The 99th percentile.
    pub p99: u64,
    /// The largest frame.
    pub max: u64,
    /// Every frame added up.
    pub sum: u64,
}

impl Spread {
    /// The spread of a build's frames.
    #[must_use]
    pub fn of(frames: &Frames) -> Self {
        let mut sizes: Vec<u64> = frames.functions.values().map(|f| f.bytes).collect();
        sizes.sort_unstable();
        let at = |q: usize| {
            sizes
                .get((sizes.len().saturating_sub(1) * q) / 100)
                .copied()
                .unwrap_or(0)
        };
        Self {
            count: sizes.len(),
            p50: at(50),
            p99: at(99),
            max: sizes.last().copied().unwrap_or(0),
            sum: sizes.iter().sum(),
        }
    }
}

/// Two builds' frames, compared.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    /// The limit used, from the reference's `.config`, or the other's when the reference has none.
    pub frame_warn: Option<u64>,
    /// Functions over the limit in the other build and not in the reference.
    pub findings: Vec<Finding>,
    /// Functions whose frame grew the most, with both sizes, the largest growth first.
    pub grown: Vec<(String, u64, u64)>,
    /// Functions both builds have.
    pub shared: usize,
    /// The spread of each side.
    pub spread: (Spread, Spread),
    /// Each side's high water mark.
    pub high_water: (Option<u64>, Option<u64>),
}

impl Comparison {
    /// The other build's high water mark over the reference's, when both booted.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn high_water_ratio(&self) -> Option<f64> {
        match self.high_water {
            (Some(r), Some(o)) if r > 0 => Some(o as f64 / r as f64),
            _ => None,
        }
    }

    /// Whether nothing is over the limit that the reference keeps under it, and the high water
    /// mark, when measured, is within 10% of the reference's.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.findings.is_empty()
            && self
                .high_water_ratio()
                .is_none_or(|r| r <= HIGH_WATER_LIMIT)
    }
}

/// Compare the other build's frames with the reference's.
#[must_use]
pub fn compare(reference: &Frames, other: &Frames) -> Comparison {
    let frame_warn = reference.frame_warn.or(other.frame_warn).filter(|w| *w > 0);
    let mut findings = Vec::new();
    let mut grown = Vec::new();
    let mut shared = 0;
    for (function, frame) in &other.functions {
        let theirs = reference.functions.get(function).map(|f| f.bytes);
        if let Some(r) = theirs {
            shared += 1;
            if frame.bytes > r {
                grown.push((function.clone(), r, frame.bytes));
            }
        }
        if let Some(limit) = frame_warn
            && frame.bytes > limit
            && theirs.is_none_or(|r| r <= limit)
        {
            findings.push(Finding {
                function: function.clone(),
                reference: theirs,
                other: frame.clone(),
            });
        }
    }
    findings.sort_by_key(|f| std::cmp::Reverse(f.other.bytes));
    grown.sort_by(|a, b| (b.2 - b.1).cmp(&(a.2 - a.1)).then(a.0.cmp(&b.0)));
    Comparison {
        frame_warn,
        findings,
        grown,
        shared,
        spread: (Spread::of(reference), Spread::of(other)),
        high_water: (reference.high_water, other.high_water),
    }
}

/// The comparison as markdown.
#[must_use]
pub fn report(c: &Comparison) -> String {
    let mut s = String::from("### frames\n\n");
    let (r, o) = c.spread;
    let _ = writeln!(
        s,
        "{} functions in the reference and {} in the other build, {} in both.\n",
        r.count, o.count, c.shared
    );
    s.push_str("| | reference | other |\n|---|---|---|\n");
    for (name, a, b) in [
        ("median frame", r.p50, o.p50),
        ("99th percentile", r.p99, o.p99),
        ("largest frame", r.max, o.max),
        ("all frames added up", r.sum, o.sum),
    ] {
        let _ = writeln!(s, "| {name} | {a} | {b} |");
    }
    let bytes = |v: Option<u64>| v.map_or_else(|| "not measured".to_string(), |b| b.to_string());
    let _ = writeln!(
        s,
        "| high water mark at run time | {} | {} |",
        bytes(c.high_water.0),
        bytes(c.high_water.1)
    );
    match c.high_water_ratio() {
        Some(ratio) => {
            let _ = writeln!(
                s,
                "\nThe high water mark is {ratio:.2} times the reference's, and the limit is {HIGH_WATER_LIMIT:.2}."
            );
        }
        None => s.push_str(
            "\nThe high water mark needs a boot.log from a kernel with CONFIG_DEBUG_STACK_USAGE on both sides.\n",
        ),
    }
    match c.frame_warn {
        None => s.push_str("\nNeither build sets CONFIG_FRAME_WARN, so no frame is over it.\n"),
        Some(limit) if c.findings.is_empty() => {
            let _ = writeln!(
                s,
                "\nNo function is over FRAME_WARN ({limit}) that the reference keeps under it."
            );
        }
        Some(limit) => {
            let _ = writeln!(
                s,
                "\n{} functions are over FRAME_WARN ({limit}) in the other build and not in the reference.\n",
                c.findings.len()
            );
            s.push_str("| function | reference | other | kind |\n|---|---|---|---|\n");
            for f in &c.findings {
                let _ = writeln!(
                    s,
                    "| {} | {} | {} | {} |",
                    f.function,
                    f.reference
                        .map_or_else(|| "none".to_string(), |b| b.to_string()),
                    f.other.bytes,
                    f.other.kind
                );
            }
        }
    }
    if !c.grown.is_empty() {
        s.push_str(
            "\nThe frames that grew the most:\n\n| function | reference | other |\n|---|---|---|\n",
        );
        for (function, a, b) in c.grown.iter().take(20) {
            let _ = writeln!(s, "| {function} | {a} | {b} |");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(files: &[(&str, &str)], frame_warn: Option<u64>) -> Frames {
        let mut f = Frames {
            frame_warn,
            ..Frames::default()
        };
        for (path, text) in files {
            add_file(&mut f.functions, path, text);
        }
        f
    }

    #[test]
    fn lines_give_the_function_and_its_frame() {
        assert_eq!(
            parse_line("kernel/fork.c:2134:13:copy_process\t392\tstatic"),
            Some((
                "copy_process".to_string(),
                Frame {
                    bytes: 392,
                    kind: "static".to_string()
                }
            ))
        );
        assert_eq!(
            parse_line("fork.c:10:f\t16\tdynamic,bounded").map(|(f, x)| (f, x.kind)),
            Some(("f".to_string(), "dynamic,bounded".to_string()))
        );
        assert_eq!(parse_line("garbage"), None);
        assert_eq!(base_name("do_idle.isra.0"), "do_idle");
        assert_eq!(object_of("kernel/fork.o.su"), "kernel/fork.o");
        assert_eq!(object_of("kernel/fork.su"), "kernel/fork.o");
    }

    #[test]
    fn a_function_and_its_clones_count_once_with_the_largest_frame() {
        let f = frames(
            &[(
                "mm/slub.su",
                "mm/slub.c:1:1:kfree\t64\tstatic\nmm/slub.c:9:1:kfree.cold\t200\tstatic\n",
            )],
            None,
        );
        assert_eq!(f.functions.len(), 1);
        assert_eq!(f.functions["mm/slub.o kfree"].bytes, 200);
    }

    #[test]
    fn a_frame_over_the_limit_that_gcc_keeps_under_it_is_a_finding() {
        let reference = frames(
            &[(
                "fs/ioctl.su",
                "fs/ioctl.c:1:1:big\t3000\tstatic\nfs/ioctl.c:2:1:mid\t1500\tstatic\nfs/ioctl.c:3:1:small\t16\tstatic\n",
            )],
            Some(2048),
        );
        let other = frames(
            &[(
                "fs/ioctl.o.su",
                "fs/ioctl.c:1:1:big\t3100\tstatic\nfs/ioctl.c:2:1:mid\t2600\tstatic\nfs/ioctl.c:3:1:small\t32\tstatic\nfs/ioctl.c:4:1:lone\t4096\tdynamic\n",
            )],
            Some(2048),
        );
        let c = compare(&reference, &other);
        assert_eq!(c.shared, 3);
        let found: Vec<&str> = c.findings.iter().map(|f| f.function.as_str()).collect();
        assert_eq!(found, ["fs/ioctl.o lone", "fs/ioctl.o mid"]);
        assert_eq!(c.grown[0], ("fs/ioctl.o mid".to_string(), 1500, 2600));
        assert!(!c.clean());
        assert!(compare(&other, &reference).clean());
        let text = report(&c);
        assert!(text.contains("| fs/ioctl.o lone | none | 4096 | dynamic |"));
        assert!(text.contains("| largest frame | 3000 | 4096 |"));
    }

    #[test]
    fn the_high_water_mark_comes_from_the_boot_log() {
        let log = "[    1.0] kworker/u4:0 (41) used greatest stack depth: 13584 bytes left\n\
                   [    2.0] sh (60) used greatest stack depth: 12880 bytes left\n";
        assert_eq!(least_left(log), Some(12880));
        let mut config = crate::kconfig::Config::new();
        config.insert("64BIT".to_string(), "y".to_string());
        assert_eq!(thread_size(&config), 16384);
        let mut reference = Frames {
            high_water: Some(3000),
            ..Frames::default()
        };
        let mut other = Frames {
            high_water: Some(3200),
            ..Frames::default()
        };
        assert!(compare(&reference, &other).clean());
        other.high_water = Some(3400);
        assert!(!compare(&reference, &other).clean());
        reference.high_water = None;
        assert!(compare(&reference, &other).clean());
    }
}
