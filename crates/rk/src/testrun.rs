//! `rk test`: the graded test run, which boots the reference kernel and the rucc kernel, runs the
//! same suites on both and compares them unit by unit (11.7 and 11.9 in the plan).
//!
//! A unit is one thing that passes or fails: `boot` for the kernel reaching the end of init with
//! no panic, `smoke:<check>` for each smoke check, `kunit:<suite>.<case>` for each KUnit result,
//! and `kunit-module:<path>` for each test module loading. Every suite is its own boot, with
//! `rk.suite=` on the kernel command line, and the kunit boot carries the build's modules in its
//! initramfs. The units the reference passes in every run are the graded set, and the rucc kernel
//! must pass every one of them in every run. A unit the reference does not always pass is
//! reported and not graded, and so is a unit of a suite in [`RACY`] and a selftest in [`TIMING`].
//!
//! Both kernels' consoles are their dmesg. The splats in them are normalized (see `dmesg`), and a
//! splat that only the rucc kernel prints is a failure even when every unit passed.
//!
//! Attribution needs two userlands, the GCC one the reference runs and the rucc one the rucc
//! kernel runs. When they differ, every suite with a failed unit runs twice more: the rucc kernel
//! with the GCC userland, and the reference kernel with the rucc userland. A unit that still fails
//! on the rucc kernel with the GCC userland is the kernel's, and goes to `rk mixed`. One that
//! passes there but fails on the reference kernel with the rucc userland is a user program
//! miscompilation. One that passes both ways is an interaction. Until rucc builds the userland
//! both kernels run the same busybox, so every failure is the kernel's.
//!
//! The selftests are units too, `kselftest:<collection>:<program>`, one boot per collection, with
//! the programs `rk selftests` built put under `/kselftest` in the initramfs with the libraries
//! they need. They are user programs, so a rucc-built set passed with `--rucc-selftests` is part
//! of the rucc userland for attribution, the way `--rucc-busybox` is.
//!
//! LTP is the same, one boot per runtest file pinned for the row, with what `rk ltp` installed for
//! that file put under `/ltp`. Every tag is a unit, `ltp:<runtest>:<tag>`, and a rucc-built LTP
//! passed with `--rucc-ltp` is part of the rucc userland too.
//!
//! What comes out is `test.json` and `summary.md` in the run directory, next to the console of
//! every boot.

use crate::boot;
use crate::personas::Row;
use crate::tap::{self, Status};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// A kind of test `rk test` can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// The kernel boots and init finishes.
    Boot,
    /// The smoke checks of `rk-init`.
    Smoke,
    /// KUnit.
    Kunit,
    /// The kernel's selftests, one boot per collection.
    Kselftest,
    /// LTP, one boot per runtest file.
    Ltp,
}

impl Kind {
    /// The word `--kinds` takes for it.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Smoke => "smoke",
            Self::Kunit => "kunit",
            Self::Kselftest => "kselftest",
            Self::Ltp => "ltp",
        }
    }
}

/// Read `--kinds`, a comma separated list.
pub fn parse_kinds(text: &str) -> Result<Vec<Kind>, String> {
    let mut kinds = Vec::new();
    for word in text.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        let kind = match word {
            "boot" => Kind::Boot,
            "smoke" => Kind::Smoke,
            "kunit" => Kind::Kunit,
            "kselftest" => Kind::Kselftest,
            "ltp" => Kind::Ltp,
            other => {
                return Err(format!(
                    "unknown test kind {other}; the kinds are boot, smoke, kunit, kselftest and ltp"
                ));
            }
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        return Err("--kinds names no kind".to_string());
    }
    kinds.sort();
    Ok(kinds)
}

/// The suites, each one boot, that the kinds need, with one `kselftest:<collection>` suite for
/// each selftest collection and one `ltp:<runtest>` suite for each runtest file.
#[must_use]
pub fn suites(kinds: &[Kind], collections: &[String], runtests: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if kinds.contains(&Kind::Boot) || kinds.contains(&Kind::Smoke) {
        out.push("smoke".to_string());
    }
    if kinds.contains(&Kind::Kunit) {
        out.push("kunit".to_string());
    }
    if kinds.contains(&Kind::Kselftest) {
        out.extend(collections.iter().map(|c| format!("kselftest:{c}")));
    }
    if kinds.contains(&Kind::Ltp) {
        out.extend(runtests.iter().map(|r| format!("ltp:{r}")));
    }
    out
}

/// The kind a unit belongs to.
#[must_use]
pub fn kind_of(unit: &str) -> Kind {
    if unit.starts_with("smoke:") {
        Kind::Smoke
    } else if unit.starts_with("kselftest:") {
        Kind::Kselftest
    } else if unit.starts_with("ltp:") {
        Kind::Ltp
    } else if unit.starts_with("kunit") {
        Kind::Kunit
    } else {
        Kind::Boot
    }
}

/// Units by name, from one boot or one run.
pub type Units = BTreeMap<String, Status>;

/// Keep the worse result of a unit that two boots both report, which is only ever `boot`.
fn merge(into: &mut Units, from: Units) {
    for (unit, status) in from {
        let entry = into.entry(unit).or_insert(status);
        *entry = (*entry).max(status);
    }
}

/// The units one boot showed, for the kinds asked for.
#[must_use]
pub fn units_of(suite: &str, kinds: &[Kind], outcome: &boot::Outcome, console: &str) -> Units {
    let mut units = Units::new();
    let booted =
        outcome.booted.is_some() && outcome.done && outcome.panic.is_none() && !outcome.timed_out;
    units.insert(
        "boot".to_string(),
        if booted { Status::Pass } else { Status::Fail },
    );
    if suite == "smoke" {
        for (check, ok) in &outcome.checks {
            units.insert(
                format!("smoke:{check}"),
                if *ok { Status::Pass } else { Status::Fail },
            );
        }
    }
    if suite == "kunit" {
        for (path, status) in tap::parse(console) {
            units.insert(format!("kunit:{path}"), status);
        }
        for line in console.lines() {
            let line = line.trim_end_matches('\r');
            if let Some((module, result)) = line
                .strip_prefix("RK-MODULE ")
                .and_then(|rest| rest.rsplit_once(' '))
            {
                let status = if result == "pass" {
                    Status::Pass
                } else {
                    Status::Fail
                };
                units.insert(format!("kunit-module:{module}"), status);
            }
        }
    }
    if suite.starts_with("kselftest:") {
        units.extend(crate::selftests::units(console));
    }
    if suite.starts_with("ltp:") {
        units.extend(crate::ltp::units(console));
    }
    units.retain(|unit, _| kinds.contains(&kind_of(unit)));
    units
}

/// One side's results over every run: each unit's status per run.
pub type Results = BTreeMap<String, Vec<Status>>;

/// Add one run's units, the run'th, so that a unit missing from some runs is `Missing` there.
pub fn add_run(results: &mut Results, run: usize, units: &Units) {
    for (unit, status) in units {
        let row = results.entry(unit.clone()).or_default();
        row.resize(run, Status::Missing);
        row.push(*status);
    }
    for row in results.values_mut() {
        row.resize(run + 1, Status::Missing);
    }
}

/// A unit in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Unit {
    /// Its name.
    pub name: String,
    /// The reference kernel's result in each run.
    pub reference: Vec<Status>,
    /// The rucc kernel's result in each run.
    pub other: Vec<Status>,
    /// Whether it is graded: the reference passed it every time, and its suite is not racy.
    pub graded: bool,
    /// Whether the rucc kernel passed it every time.
    pub passed: bool,
    /// For a graded unit the rucc kernel failed: `kernel`, `userland` or `interaction`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
}

/// KUnit suites whose units are reported and not graded, because the test races with the kernel
/// itself and a failure tells nothing about the compiler.
///
/// Each `*_iommu_test` suite in `iommu_pt_kunit_test.ko` reads `NR_SECONDARY_PAGETABLE` before a
/// case and checks that it is the same after. The counter is the global one, and the vmstat worker
/// folds each CPU's part into it once a second. When the worker runs during a case, the counter
/// moves by the pages the case had, the check fails, and the cleanup stops before it unregisters
/// `pt_kunit_dev`, so the suites after it fail too. The GCC kernel fails the same way with a slower
/// userland.
///
/// `cs35l56_test_parse_xu_onchip_spkid` in `snd-soc-cs35l56-test-soundwire` registers a software
/// node it allocated with `kunit_kzalloc` for each of its cases. With `CONFIG_DEBUG_KOBJECT_RELEASE`,
/// which the 7.2.8 Fedora config has, the node of the case before stays registered for one to four
/// seconds after it is put, so when the next case gets the same address `software_node_register`
/// returns `-EEXIST`. On the 7.2.8 arm64 nightly the rucc kernel failed four of the cases this
/// way and passed all twenty with the GCC userland.
///
/// `dm_test_process_output_property_validate_always_scheduled` in `amdgpu_dm_hdcp` has
/// `process_output` queue a work with no delay on the system workqueue and then checks that the
/// work is still pending. With two CPUs a worker on the other one can run the empty work first.
/// On the 7.2.8 arm64 nightly the reference kernel with the rucc modules failed it this way, and
/// the same modules passed it in the rucc kernel.
const RACY: &[&str] = &[
    "_iommu_test",
    "snd-soc-cs35l56-test-soundwire",
    "amdgpu_dm_hdcp",
];

/// Tests whose result is a measurement of time, which are reported and not graded, because under
/// emulation the host decides how long anything takes.
///
/// `seccomp_benchmark` checks that the cost of one more filter worked out two ways comes out
/// about the same, and on the 6.12.111 arm64 nightly the rucc kernel measured 469 ns one way and
/// 385 ns the other, where the test itself says to try again with more samples. `rtcpie` checks
/// that the periodic RTC interrupts come on time, and the rucc kernel with the rucc userland
/// measured 0.0173 s between two of them at 64 Hz, where 0.0156 s is expected. It passed with the
/// GCC userland. `getrusage04` checks that the user time goes up by no more than 5000 us between
/// two calls a timer tick apart, and on the same nightly the rucc kernel saw it go up by 5140 us
/// once in twenty. It passed both ways when it ran again with the other userland.
///
/// Some LTP tests have to get a fixed amount of work done in a fixed time. `fcntl14` and
/// `fcntl14_64` fork a child for each of 5000 locks and have 38 seconds for it. On the 6.12.111
/// arm64 nightly with `-cpu cortex-a57` they ran out of time on the rucc kernel with either
/// userland, and also on the reference kernel with the rucc userland. `futex_cmp_requeue01` forks
/// 1000 waiters that each give up after five seconds. On the same night, with the rucc kernel and
/// the rucc userland, every waiter gave up before the last was forked, and it passed in both
/// crossed runs.
///
/// `shm_test01` attaches a segment of a random size between one and a thousand pages ten times
/// and calls `sched_yield` once for each byte it writes or reads, with the seed taken from the
/// time. On the 7.2.8 arm64 nightly it took under 160 seconds on the reference kernel and more
/// than the 600 LTP allows on the rucc kernel with the rucc userland, and it passed in both
/// crossed runs, which took 250 and 430 seconds.
const TIMING: &[&str] = &[
    "kselftest:seccomp:seccomp_benchmark",
    "kselftest:timers:rtcpie",
    "ltp:mm:shm_test01",
    "ltp:syscalls:fcntl14",
    "ltp:syscalls:fcntl14_64",
    "ltp:syscalls:futex_cmp_requeue01",
    "ltp:syscalls:getrusage04",
];

/// KUnit units the test itself expects to fail, which are reported and not graded, because
/// whether they pass is luck.
///
/// `stackinit` fills the stack from one call, then reads the variable a second call never sets.
/// The two `switch` units declare it before the first `case`, where no compiler can set it, and
/// skip when they find the fill. They pass only when something wrote over the dead stack in
/// between, such as the registers an exception saves on arm64 before it moves to the IRQ stack.
/// On the 7.2.8 arm64 nightly the reference passed `test_switch_2_none` and the rucc kernel
/// skipped it, and `rk mixed` found that the reference skips it too on another boot.
const ALWAYS_FAIL: &[&str] = &[
    "kunit:stackinit.test_switch_1_none",
    "kunit:stackinit.test_switch_2_none",
];

/// Whether a unit is in a suite of [`RACY`] or is one of [`TIMING`] or [`ALWAYS_FAIL`].
pub fn racy(name: &str) -> bool {
    TIMING.contains(&name)
        || ALWAYS_FAIL.contains(&name)
        || name
            .strip_prefix("kunit:")
            .and_then(|rest| rest.split('.').next())
            .is_some_and(|suite| RACY.iter().any(|end| suite.ends_with(end)))
}

/// Grade the rucc kernel's results against the reference's over `runs` runs.
#[must_use]
pub fn grade(reference: &Results, other: &Results, runs: usize) -> Vec<Unit> {
    let names: BTreeSet<&String> = reference.keys().chain(other.keys()).collect();
    let missing = vec![Status::Missing; runs];
    names
        .into_iter()
        .map(|name| {
            let r = reference.get(name).unwrap_or(&missing);
            let o = other.get(name).unwrap_or(&missing);
            Unit {
                name: name.clone(),
                reference: r.clone(),
                other: o.clone(),
                graded: !racy(name) && r.len() == runs && r.iter().all(|s| *s == Status::Pass),
                passed: o.len() == runs && o.iter().all(|s| *s == Status::Pass),
                attribution: None,
            }
        })
        .collect()
}

/// Where a failure comes from, given the two cells of the 2x2 that were run for it: the rucc
/// kernel with the GCC userland, and the reference kernel with the rucc userland.
#[must_use]
pub fn attribute(
    rucc_kernel_gcc_userland: Status,
    gcc_kernel_rucc_userland: Status,
) -> &'static str {
    if rucc_kernel_gcc_userland != Status::Pass {
        "kernel"
    } else if gcc_kernel_rucc_userland != Status::Pass {
        "userland"
    } else {
        "interaction"
    }
}

/// Everything `rk test` was asked to do.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The reference build directory.
    pub reference: PathBuf,
    /// The rucc build directory.
    pub other: PathBuf,
    /// The row both were built for.
    pub row: Row,
    /// The GCC userland: a static busybox.
    pub busybox: Vec<u8>,
    /// The rucc userland, when there is one. Without it both kernels run `busybox`.
    pub rucc_busybox: Option<Vec<u8>>,
    /// The GCC-built selftests, an `rk selftests` output directory.
    pub selftests: Option<PathBuf>,
    /// The rucc-built selftests. Without them both kernels run `selftests`.
    pub rucc_selftests: Option<PathBuf>,
    /// The GCC-built LTP, an `rk ltp` output directory.
    pub ltp: Option<PathBuf>,
    /// The rucc-built LTP. Without it both kernels run `ltp`.
    pub rucc_ltp: Option<PathBuf>,
    /// The kinds of test.
    pub kinds: Vec<Kind>,
    /// How many times each kernel runs every suite.
    pub runs: usize,
    /// Seconds each boot may take.
    pub timeout: u64,
    /// How many boots run at once.
    pub jobs: usize,
    /// The run directory.
    pub out: PathBuf,
}

/// Modules that rk test leaves out for both kernels, because a splat or a hang they cause tells
/// nothing about the compiler. The KUnit ones race with themselves, and `KUNIT_ALL_TESTS=m` hides
/// their prompts, so the fragment cannot turn them off.
///
/// `test_ratelimit_stress` sets a flag that its threads stop on and then calls `kthread_stop` on
/// them. A thread that sees the flag and exits first frees its task, and `kthread_stop` then
/// reports `refcount_t: addition on 0`. Upstream fixes the test with `kthread_should_stop()`.
///
/// The mock scheduler of `drm-sched-tests` signals a job's fence from the job's hrtimer, with its
/// lock held and interrupts off. When that drops the last reference to the fence, the release
/// calls `hrtimer_cancel` on the timer that is running, which waits for itself, and the other CPU
/// then waits for the lock. The guest prints nothing more, and every suite after it fails. A GCC
/// kernel hangs the same way.
///
/// A speakup synth built as a module starts on load and talks to its synthesizer over `ttyS0`.
/// Debian's and Fedora's arm64 configs both build them. Once the first one, `speakup_acntsa`, has
/// started, `speakup_apollo` waits for speakup's lock in `synth_add` until the boot times out, and
/// both kernels hang there on 6.12.111 and on 7.2.8. `speakup_soft` needs no port and stays.
///
/// `locktorture` starts its writer threads on load and runs them until it is unloaded. With
/// Fedora's arm64 config on 7.2.8 the kunit boot had loaded 3600 modules in 1890 seconds when it
/// came to `locktorture`, and the next 616 took as long again.
///
/// `rcutorture` also starts its threads on load. After each stutter its writer warns at
/// `rcutorture.c:1950` if no element has come back from a grace period. That is a check of time,
/// and the console load of the kunit boot sets the time. With Fedora's x86-64 config on 7.3 the
/// `drm_mode_probed_add` warnings of `amdgpu_dm_wb` print the list of loaded modules 32 times, and
/// in that window both kernels completed only about 30 grace periods in 60 seconds. The rucc
/// kernel gave the warning in each of three boots, and the gcc kernel did not give it.
///
/// `snd-mts64` and `snd-portman2x4` are MIDI interfaces on the parallel port, and Fedora's x86-64
/// config builds both. Under QEMU, `snd-mts64` either fails its probe or spins in it until the soft
/// lockup detector reports it, and `snd-portman2x4` registers its interrupt handler before it sets
/// the card's `private_data`. The parallel port interrupt then comes first, and the handler takes a
/// lock through a null pointer. On 7.3 the kunit boot of both kernels stopped there.
///
/// `dm_test_crtc_enable_vblank_ips_restore` in `amdgpu_dm_crtc_test` makes the enable path call
/// `drm_crtc_vblank_restore` without `vbl_lock` held, and `assert_spin_locked` in
/// `drm_vblank_restore` is then a `BUG`. With Fedora's x86-64 config on 7.3 both kernels stopped
/// there, and the suites that each had not run yet were missing from its boot.
const LEFT_OUT: &[&str] = &[
    "sound/drivers/snd-mts64.ko",
    "sound/drivers/snd-portman2x4.ko",
    "lib/tests/test_ratelimit.ko",
    "drivers/gpu/drm/scheduler/tests/drm-sched-tests.ko",
    "drivers/accessibility/speakup/speakup_acntsa.ko",
    "drivers/accessibility/speakup/speakup_apollo.ko",
    "drivers/accessibility/speakup/speakup_audptr.ko",
    "drivers/accessibility/speakup/speakup_bns.ko",
    "drivers/accessibility/speakup/speakup_decext.ko",
    "drivers/accessibility/speakup/speakup_dectlk.ko",
    "drivers/accessibility/speakup/speakup_dummy.ko",
    "drivers/accessibility/speakup/speakup_ltlk.ko",
    "drivers/accessibility/speakup/speakup_spkout.ko",
    "drivers/accessibility/speakup/speakup_txprt.ko",
    "kernel/locking/locktorture.ko",
    "kernel/rcu/rcutorture.ko",
    "drivers/gpu/drm/amd/amdgpu/../display/amdgpu_dm/tests/amdgpu_dm_crtc_test.ko",
];

/// The modules of a build as files for the initramfs, under `lib/modules/rk`, and the `order`
/// file listing them in the order kbuild built them, without the ones in [`LEFT_OUT`]. Each is
/// without its debug information, which a distribution config puts gigabytes of in, so that the
/// initramfs stays under what QEMU reads. With `crossed`, for a kernel the modules were not
/// built with, each is without its BTF too. See [`crate::modules::without_debug_or_btf`].
pub fn modules(build: &Path, crossed: bool) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = Vec::new();
    let mut names = String::new();
    for name in module_names(build) {
        let bytes = read_module(build, &name)?;
        files.push((
            format!("lib/modules/rk/{name}"),
            if crossed {
                crate::modules::without_debug_or_btf(&bytes)
            } else {
                crate::modules::without_debug(&bytes)
            },
        ));
        let _ = writeln!(names, "{name}");
    }
    if !files.is_empty() {
        files.push(("lib/modules/rk/order".to_string(), names.into_bytes()));
    }
    Ok(files)
}

/// The modules of a build by their path in the build, in the order kbuild built them, without
/// the ones in [`LEFT_OUT`]. None when the build has no `modules.order`.
#[must_use]
pub fn module_names(build: &Path) -> Vec<String> {
    let Ok(order) = std::fs::read_to_string(build.join("modules.order")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in order.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let stem = line
            .strip_suffix(".ko")
            .or_else(|| line.strip_suffix(".o"))
            .unwrap_or(line);
        let name = format!("{stem}.ko");
        if !LEFT_OUT.contains(&name.as_str()) {
            out.push(name);
        }
    }
    out
}

/// Seconds a kunit boot may take: `base`, and one more for each module it loads, since a distro
/// config has thousands. With Fedora's arm64 config the reference kernel of 7.2.8 had loaded 4216
/// of its 5834 modules when the 2400 seconds A64 gives a boot under TCG ran out.
#[must_use]
pub fn kunit_timeout(base: u64, files: &[(String, Vec<u8>)]) -> u64 {
    let modules = files
        .iter()
        .filter(|(name, _)| Path::new(name).extension().is_some_and(|e| e == "ko"))
        .count();
    base + u64::try_from(modules).unwrap_or(u64::MAX / 4)
}

/// One module of a build, by its name from [`module_names`].
pub fn read_module(build: &Path, name: &str) -> Result<Vec<u8>, String> {
    let path = build.join(name);
    std::fs::read(&path).map_err(|e| {
        format!(
            "reading {}: {e}; build the modules too, with --targets \"<image> modules\"",
            path.display()
        )
    })
}

impl Plan {
    /// The suites to boot, with the selftest collections and runtest files that have something
    /// built.
    fn suites(&self) -> Result<Vec<String>, String> {
        let collections = match &self.selftests {
            Some(dir) if self.kinds.contains(&Kind::Kselftest) => {
                crate::selftests::Outcome::load(dir)?.runnable()
            }
            _ => Vec::new(),
        };
        let runtests = match &self.ltp {
            Some(dir) if self.kinds.contains(&Kind::Ltp) => {
                crate::ltp::Outcome::load(dir)?.runnable()
            }
            _ => Vec::new(),
        };
        Ok(suites(&self.kinds, &collections, &runtests))
    }

    /// The GCC userland and the rucc one, which is the GCC one where nothing else was given.
    fn userlands(&self) -> (Userland<'_>, Userland<'_>) {
        let gcc = Userland {
            busybox: &self.busybox,
            selftests: self.selftests.as_deref(),
            ltp: self.ltp.as_deref(),
        };
        let rucc = Userland {
            busybox: self.rucc_busybox.as_deref().unwrap_or(&self.busybox),
            selftests: self.rucc_selftests.as_deref().or(gcc.selftests),
            ltp: self.rucc_ltp.as_deref().or(gcc.ltp),
        };
        (gcc, rucc)
    }
}

/// The kind of the units a suite reports, apart from `boot`, which every suite reports.
#[must_use]
pub fn suite_kind(suite: &str) -> Kind {
    match suite {
        "smoke" => Kind::Smoke,
        "kunit" => Kind::Kunit,
        _ if suite.starts_with("kselftest:") => Kind::Kselftest,
        _ if suite.starts_with("ltp:") => Kind::Ltp,
        _ => Kind::Boot,
    }
}

/// A userland: the busybox and, for the selftests and LTP, the output directories of
/// `rk selftests` and `rk ltp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Userland<'a> {
    busybox: &'a [u8],
    selftests: Option<&'a Path>,
    ltp: Option<&'a Path>,
}

/// What one boot reported and the splats in its log.
type Booted = Result<(Units, BTreeSet<String>), String>;

/// Every boot of `boots`, as kernel, userland, suite and stem, `plan.jobs` at a time, with what
/// each found in the same order.
fn boot_all<'a>(
    plan: &'a Plan,
    boots: Vec<(&'a Path, Userland<'a>, &'a str, String)>,
) -> Vec<Booted> {
    let tasks = boots
        .into_iter()
        .map(
            |(build, userland, suite, stem)| -> crate::pool::Task<'a, Booted> {
                Box::new(move || boot_suite(plan, build, userland, suite, &stem))
            },
        )
        .collect();
    crate::pool::run(plan.jobs, tasks)
}

/// One boot of one kernel with one userland running one suite, named by `stem` in the run
/// directory. The initramfs is removed after the boot, and the console is kept.
fn boot_suite(plan: &Plan, build: &Path, userland: Userland, suite: &str, stem: &str) -> Booted {
    let files = if suite == "kunit" {
        modules(build, false)?
    } else if let Some(collection) = suite.strip_prefix("kselftest:") {
        let dir = userland
            .selftests
            .ok_or("the kselftest kind needs --selftests, an rk selftests directory")?;
        crate::selftests::files(dir, collection)?
    } else if let Some(runtest) = suite.strip_prefix("ltp:") {
        let dir = userland
            .ltp
            .ok_or("the ltp kind needs --ltp, an rk ltp directory")?;
        crate::ltp::files(dir, runtest)?
    } else {
        Vec::new()
    };
    let timeout = if suite.starts_with("ltp:") {
        crate::ltp::timeout(plan.timeout, &files)
    } else if suite == "kunit" {
        kunit_timeout(plan.timeout, &files)
    } else {
        plan.timeout
    };
    let busybox = userland.busybox;
    let initramfs = plan.out.join(format!("{stem}.cpio"));
    std::fs::write(&initramfs, boot::initramfs_with(busybox, &files))
        .map_err(|e| format!("writing {}: {e}", initramfs.display()))?;
    let log = plan.out.join(format!("{stem}.log"));
    let booted = (|| -> Result<_, String> {
        for boots in 1.. {
            eprintln!("rk: {stem}");
            let outcome = boot::run(&boot::Plan {
                build: build.to_path_buf(),
                row: plan.row.clone(),
                initramfs: initramfs.clone(),
                timeout,
                append: format!("rk.suite={suite}"),
                memory: crate::ltp::memory(suite),
                stem: Some(plan.out.join(stem)),
            })?;
            let console = std::fs::read_to_string(&log)
                .map_err(|e| format!("reading {}: {e}", log.display()))?;
            if boots == CLOCK_BOOTS || !wrong_clock(&console) {
                return Ok((outcome, console));
            }
            eprintln!("rk: {stem} took its TSC frequency from a bad PIT sample, booting it again");
        }
        unreachable!("the boots stop at CLOCK_BOOTS")
    })();
    let _ = std::fs::remove_file(&initramfs);
    let (outcome, console) = booted?;
    Ok((
        units_of(suite, &plan.kinds, &outcome, &console),
        crate::dmesg::splats(&console),
    ))
}

/// How many times a suite boots when the kernel's clock is wrong each time.
const CLOCK_BOOTS: usize = 3;

/// Whether an x86 kernel took the TSC frequency from a PIT sample that the HPET or the PM timer
/// does not agree with. Under TCG on a loaded host the vCPU can stop in the PIT loop, and a sample
/// can pass the checks with a TSC count that is too small. The kernel then uses the PIT value
/// because it trusts the PIT more than the reference. With `tsc=nowatchdog` nothing corrects
/// it later, and the clock of the kernel runs too fast. The rucc kernel of the 7.3 UBSAN defconfig
/// took 133.966 MHz where the HPET said 2794.851 MHz, its clock ran about 21 times too fast,
/// and six KUnit cases timed out. The gcc and rucc kernels have the same code there, and either
/// can draw the bad sample.
fn wrong_clock(console: &str) -> bool {
    console
        .lines()
        .any(|line| line.contains("tsc: PIT calibration deviates from"))
        && console
            .lines()
            .any(|line| line.contains("tsc: Using PIT calibration value"))
}

/// The splats of both kernels over every run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Splats {
    /// The reference kernel's.
    pub reference: BTreeSet<String>,
    /// The rucc kernel's.
    pub other: BTreeSet<String>,
    /// The rucc kernel's that the reference never printed, in every run.
    pub other_only: BTreeSet<String>,
    /// The rucc kernel's that the reference never printed, in some runs but not in all of them.
    pub racy: BTreeSet<String>,
}

/// The splats of both kernels, with the rucc kernel's own put apart from the racy ones.
///
/// A rucc splat is also the reference's when the reference printed the same splat, or the same
/// splat in a helper that one of the compilers inlined (see `dmesg::same`). Over more than one
/// run, a rucc splat that is not in the log of each run is racy. A timer interrupt or the KASAN
/// quarantine can make such a splat come and go. An example is `wild-memory-access in
/// _raw_spin_lock`, which KASAN prints when an interrupt comes while `identify_cpu` has cleared
/// `X86_FEATURE_LA57`. The summary shows a racy splat, but it does not fail the rucc kernel,
/// because wrong code usually gives its splat in each run.
fn sort_splats(reference: BTreeSet<String>, other: &[BTreeSet<String>]) -> Splats {
    let mut splats = Splats {
        reference,
        other: other.iter().flatten().cloned().collect(),
        ..Splats::default()
    };
    for splat in &splats.other {
        if splats
            .reference
            .iter()
            .any(|r| crate::dmesg::same(splat, r))
        {
            continue;
        }
        if other.len() > 1 && other.iter().any(|run| !run.contains(splat)) {
            splats.racy.insert(splat.clone());
        } else {
            splats.other_only.insert(splat.clone());
        }
    }
    splats
}

/// What `rk test` found, written to `test.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// The reference build directory.
    pub reference: PathBuf,
    /// The rucc build directory.
    pub other: PathBuf,
    /// The kinds of test.
    pub kinds: Vec<Kind>,
    /// Runs of each suite on each kernel.
    pub runs: usize,
    /// Whether the rucc kernel ran its own userland, so that failures could be attributed.
    pub two_userlands: bool,
    /// Every unit either kernel reported.
    pub units: Vec<Unit>,
    /// The splats.
    pub splats: Splats,
}

impl Outcome {
    /// The graded units the rucc kernel failed.
    #[must_use]
    pub fn failed(&self) -> Vec<&Unit> {
        self.units
            .iter()
            .filter(|u| u.graded && !u.passed)
            .collect()
    }

    /// Whether the rucc kernel passed every graded unit and printed no splat of its own.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failed().is_empty() && self.splats.other_only.is_empty()
    }
}

/// Run every suite on both kernels, grade, attribute, and write `test.json` and `summary.md`.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    std::fs::create_dir_all(&plan.out)
        .map_err(|e| format!("creating {}: {e}", plan.out.display()))?;
    let (gcc, rucc) = plan.userlands();
    let two_userlands = gcc != rucc;
    let suites = plan.suites()?;
    let mut reference = Results::new();
    let mut other = Results::new();
    let mut reference_splats = BTreeSet::new();
    let mut other_splats = Vec::new();
    for run in 0..plan.runs {
        let n = run + 1;
        // The two kernels boot each suite at the same time, so that whatever else the host is
        // doing slows both of them alike.
        let mut boots = Vec::new();
        for suite in &suites {
            let name = stem(suite);
            boots.push((
                plan.reference.as_path(),
                gcc,
                suite.as_str(),
                format!("reference-{name}-{n}"),
            ));
            boots.push((
                plan.other.as_path(),
                rucc,
                suite.as_str(),
                format!("other-{name}-{n}"),
            ));
        }
        let mut booted = boot_all(plan, boots).into_iter();
        let (mut r_units, mut o_units) = (Units::new(), Units::new());
        let mut o_splats = BTreeSet::new();
        while let (Some(reference_boot), Some(other_boot)) = (booted.next(), booted.next()) {
            let (units, found) = reference_boot?;
            merge(&mut r_units, units);
            reference_splats.extend(found);
            let (units, found) = other_boot?;
            merge(&mut o_units, units);
            o_splats.extend(found);
        }
        add_run(&mut reference, run, &r_units);
        add_run(&mut other, run, &o_units);
        other_splats.push(o_splats);
    }
    let splats = sort_splats(reference_splats, &other_splats);
    let mut outcome = Outcome {
        reference: plan.reference.clone(),
        other: plan.other.clone(),
        kinds: plan.kinds.clone(),
        runs: plan.runs,
        two_userlands,
        units: grade(&reference, &other, plan.runs),
        splats,
    };
    // Written once before the boots that attribute the failures, so that a job that runs out of
    // time in them still has the grades.
    write(plan, &outcome)?;

    let failed: Vec<&str> = outcome.failed().iter().map(|u| u.name.as_str()).collect();
    let (mut swapped_kernel, mut swapped_userland) = (Units::new(), Units::new());
    if two_userlands && !failed.is_empty() {
        let mut boots = Vec::new();
        for suite in &suites {
            if !rerun(suite, &failed) {
                continue;
            }
            let name = stem(suite);
            boots.push((
                plan.other.as_path(),
                gcc,
                suite.as_str(),
                format!("other-gcc-userland-{name}"),
            ));
            boots.push((
                plan.reference.as_path(),
                rucc,
                suite.as_str(),
                format!("reference-rucc-userland-{name}"),
            ));
        }
        let mut booted = boot_all(plan, boots).into_iter();
        while let (Some(kernel_boot), Some(userland_boot)) = (booted.next(), booted.next()) {
            merge(&mut swapped_kernel, kernel_boot?.0);
            merge(&mut swapped_userland, userland_boot?.0);
        }
    }
    for unit in outcome.units.iter_mut().filter(|u| u.graded && !u.passed) {
        unit.attribution = Some(if two_userlands {
            let cell = |units: &Units| units.get(&unit.name).copied().unwrap_or(Status::Missing);
            attribute(cell(&swapped_kernel), cell(&swapped_userland)).to_string()
        } else {
            "kernel".to_string()
        });
    }

    write(plan, &outcome)?;
    Ok(outcome)
}

/// Write `test.json` and `summary.md` for the outcome.
fn write(plan: &Plan, outcome: &Outcome) -> Result<(), String> {
    let json = serde_json::to_string_pretty(outcome).unwrap_or_default();
    std::fs::write(plan.out.join("test.json"), json + "\n")
        .map_err(|e| format!("writing test.json: {e}"))?;
    std::fs::write(plan.out.join("summary.md"), summary(outcome))
        .map_err(|e| format!("writing summary.md: {e}"))
}

/// Whether a suite boots again with the userlands swapped, to attribute these failed units. A
/// failed boot could have been any suite's, so then every suite does. Otherwise only a suite that
/// reports one of the units does: an LTP test that failed in `syscalls` says nothing about `mm`,
/// and booting every runtest file again took the A64 job for 7.2.8 past its limit. A KUnit case
/// runs in the kernel, so the userland cannot be why it failed and it is the kernel's without a
/// second boot. The KUnit boot of 7.2.8 takes about two hours on arm64, and booting it again for
/// one case ran the job past its limit. A test module that did not load still boots again, since
/// it is the userland that loads it.
#[must_use]
pub fn rerun(suite: &str, failed: &[&str]) -> bool {
    failed.iter().any(|unit| {
        if unit.starts_with("kunit:") {
            return false;
        }
        let kind = kind_of(unit);
        kind == Kind::Boot
            || match kind {
                Kind::Kselftest | Kind::Ltp => unit
                    .strip_prefix(suite)
                    .is_some_and(|rest| rest.starts_with(':')),
                kind => suite_kind(suite) == kind,
            }
    })
}

/// A suite as part of a file name, without the colon of `kselftest:<collection>`.
fn stem(suite: &str) -> String {
    suite.replace(':', "-")
}

/// A unit's results over the runs, as words.
fn statuses(list: &[Status]) -> String {
    list.iter().map(|s| s.word()).collect::<Vec<_>>().join(" ")
}

/// The outcome as markdown.
#[must_use]
pub fn summary(o: &Outcome) -> String {
    let mut s = String::from("### rk test\n\n");
    let kinds: Vec<&str> = o.kinds.iter().map(|k| k.word()).collect();
    let graded = o.units.iter().filter(|u| u.graded).count();
    let failed = o.failed();
    let _ = writeln!(
        s,
        "{} over {} runs: the reference passed {graded} units every time, and the rucc kernel passed {} of them. {} splats are only in the rucc kernel's log. {}.\n",
        kinds.join(", "),
        o.runs,
        graded - failed.len(),
        o.splats.other_only.len(),
        if o.passed() { "Passed" } else { "Failed" }
    );
    if !o.two_userlands {
        s.push_str("Both kernels ran the same userland, so every failure is the kernel's.\n\n");
    }
    if !failed.is_empty() {
        s.push_str("| unit | reference | rucc | attribution |\n|---|---|---|---|\n");
        for u in failed.iter().take(200) {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                u.name.replace('|', "\\|"),
                statuses(&u.reference),
                statuses(&u.other),
                u.attribution.as_deref().unwrap_or("")
            );
        }
        if failed.len() > 200 {
            let _ = writeln!(s, "\nand {} more.", failed.len() - 200);
        }
        s.push('\n');
    }
    if !o.splats.other_only.is_empty() {
        s.push_str("Splats only in the rucc kernel's log:\n\n");
        for splat in &o.splats.other_only {
            let _ = writeln!(s, "- `{}`", splat.replace('`', "'"));
        }
        s.push('\n');
    }
    if !o.splats.racy.is_empty() {
        s.push_str("Splats only in the rucc kernel's log, in some runs but not in all of them. They do not fail the run:\n\n");
        for splat in &o.splats.racy {
            let _ = writeln!(s, "- `{}`", splat.replace('`', "'"));
        }
        s.push('\n');
    }
    let ungraded: Vec<&Unit> = o.units.iter().filter(|u| !u.graded).collect();
    if !ungraded.is_empty() {
        let _ = writeln!(
            s,
            "{} units are not graded because the reference did not pass them every time or their test is racy, and the rucc kernel passed {} of those.",
            ungraded.len(),
            ungraded.iter().filter(|u| u.passed).count()
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_read_and_the_later_ones_refused() {
        assert_eq!(
            parse_kinds("kunit,boot,smoke,boot").unwrap(),
            [Kind::Boot, Kind::Smoke, Kind::Kunit]
        );
        assert_eq!(
            parse_kinds("boot,kselftest").unwrap(),
            [Kind::Boot, Kind::Kselftest]
        );
        assert_eq!(parse_kinds("ltp,boot").unwrap(), [Kind::Boot, Kind::Ltp]);
        assert!(parse_kinds("fuzz").unwrap_err().contains("unknown"));
        let timers = ["timers".to_string(), "size".to_string()];
        let syscalls = ["syscalls".to_string()];
        assert_eq!(suites(&[Kind::Boot], &timers, &syscalls), ["smoke"]);
        assert_eq!(suites(&[Kind::Kunit], &timers, &syscalls), ["kunit"]);
        assert_eq!(
            suites(&[Kind::Kselftest], &timers, &syscalls),
            ["kselftest:timers", "kselftest:size"]
        );
        assert_eq!(suites(&[Kind::Ltp], &timers, &syscalls), ["ltp:syscalls"]);
        assert_eq!(kind_of("ltp:syscalls:abort01"), Kind::Ltp);
        assert_eq!(suite_kind("ltp:syscalls"), Kind::Ltp);
        assert_eq!(kind_of("kselftest:timers:posix_timers"), Kind::Kselftest);
        assert_eq!(suite_kind("kselftest:timers"), Kind::Kselftest);
        assert_eq!(suite_kind("kunit"), Kind::Kunit);
    }

    #[test]
    fn a_kernel_that_trusted_a_bad_pit_sample_boots_again() {
        let bad = "[    0.032000] tsc: PIT calibration deviates from HPET: 133966 2794851\n\
                   [    0.033000] tsc: Using PIT calibration value\n\
                   [    0.033000] tsc: Detected 133.966 MHz processor\n";
        assert!(wrong_clock(bad));
        let good = "[    0.028000] tsc: Unable to calibrate against PIT\n\
                    [    0.029000] tsc: using HPET reference calibration\n\
                    [    0.029000] tsc: Detected 2794.860 MHz processor\n";
        assert!(!wrong_clock(good));
        assert!(!wrong_clock(""));
    }

    #[test]
    fn only_a_suite_with_a_failed_unit_boots_again() {
        let failed = ["ltp:syscalls:fcntl14", "kunit-module:crypto/xor.ko"];
        assert!(rerun("ltp:syscalls", &failed));
        assert!(!rerun("ltp:syscalls-ipc", &failed));
        assert!(!rerun("ltp:mm", &failed));
        assert!(rerun("kunit", &failed));
        assert!(!rerun("smoke", &failed));
        assert!(!rerun("kselftest:timers", &failed));
        assert!(rerun(
            "kselftest:timers",
            &["kselftest:timers:posix_timers"]
        ));
        assert!(!rerun(
            "kselftest:timers",
            &["kselftest:timens:clock_nanosleep"]
        ));
        assert!(rerun("smoke", &["smoke:mount"]));
        assert!(rerun("ltp:mm", &["boot"]));
        assert!(!rerun(
            "kunit",
            &["kunit:list-kunit-test.list_test_list_init"]
        ));
        assert!(rerun(
            "kunit",
            &[
                "kunit:list-kunit-test.list_test_list_init",
                "kunit-module:lib/test_list.ko"
            ]
        ));
    }

    #[test]
    fn a_kunit_boot_gets_a_second_more_for_each_module() {
        let files = vec![
            ("lib/modules/rk/a.ko".to_string(), Vec::new()),
            ("lib/modules/rk/b.ko".to_string(), Vec::new()),
            ("lib/modules/rk/order".to_string(), Vec::new()),
        ];
        assert_eq!(kunit_timeout(600, &files), 602);
        assert_eq!(kunit_timeout(600, &[]), 600);
    }

    #[test]
    fn a_racy_kunit_module_is_left_out_of_the_initramfs() {
        let dir = std::env::temp_dir().join(format!("rk-testrun-modules-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lib/tests")).unwrap();
        std::fs::write(dir.join("lib/tests/list-test.ko"), b"list").unwrap();
        std::fs::write(dir.join("lib/tests/test_ratelimit.ko"), b"race").unwrap();
        let sched = dir.join("drivers/gpu/drm/scheduler/tests");
        std::fs::create_dir_all(&sched).unwrap();
        std::fs::write(sched.join("drm-sched-tests.ko"), b"hang").unwrap();
        std::fs::write(
            dir.join("modules.order"),
            "lib/tests/test_ratelimit.o\ndrivers/gpu/drm/scheduler/tests/drm-sched-tests.o\nlib/tests/list-test.o\n",
        )
        .unwrap();
        let files = modules(&dir, false).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "lib/modules/rk/lib/tests/list-test.ko",
                "lib/modules/rk/order"
            ]
        );
        assert_eq!(files[1].1, b"lib/tests/list-test.ko\n");
    }

    #[test]
    fn a_speakup_synth_that_needs_a_port_is_left_out() {
        let dir = std::env::temp_dir().join(format!("rk-testrun-speakup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let speakup = "drivers/accessibility/speakup";
        std::fs::write(
            dir.join("modules.order"),
            format!(
                "{speakup}/speakup.o\n{speakup}/speakup_acntsa.o\n{speakup}/speakup_apollo.o\n{speakup}/speakup_soft.o\n"
            ),
        )
        .unwrap();
        let names = module_names(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            names,
            [
                format!("{speakup}/speakup.ko"),
                format!("{speakup}/speakup_soft.ko")
            ]
        );
    }

    fn booted() -> boot::Outcome {
        boot::Outcome {
            booted: Some("7.2.8".to_string()),
            done: true,
            checks: vec![("fork".to_string(), true), ("pipe".to_string(), false)],
            ..boot::Outcome::default()
        }
    }

    #[test]
    fn a_boot_gives_units_for_the_kinds_asked_for() {
        let units = units_of("smoke", &[Kind::Boot, Kind::Smoke], &booted(), "");
        assert_eq!(units["boot"], Status::Pass);
        assert_eq!(units["smoke:pipe"], Status::Fail);
        let only_boot = units_of("smoke", &[Kind::Boot], &booted(), "");
        assert_eq!(only_boot.len(), 1);
        let console = "RK-BOOTED 7.2.8 1.0\n[ 1.0]     # Subtest: list\n[ 1.0]     ok 1 list_test\n[ 1.0] ok 1 list\nRK-MODULE lib/list-test.ko pass\nRK-MODULE lib/bad.ko fail\n";
        let units = units_of("kunit", &[Kind::Kunit], &booted(), console);
        assert_eq!(units["kunit:list.list_test"], Status::Pass);
        assert_eq!(units["kunit-module:lib/bad.ko"], Status::Fail);
        assert!(!units.contains_key("boot"));
    }

    #[test]
    fn a_unit_the_reference_always_passes_is_graded() {
        let mut reference = Results::new();
        let mut other = Results::new();
        let run = |pairs: &[(&str, Status)]| -> Units {
            pairs.iter().map(|(u, s)| ((*u).to_string(), *s)).collect()
        };
        add_run(
            &mut reference,
            0,
            &run(&[
                ("boot", Status::Pass),
                ("kunit:a", Status::Pass),
                ("kunit:flaky", Status::Fail),
            ]),
        );
        add_run(
            &mut reference,
            1,
            &run(&[
                ("boot", Status::Pass),
                ("kunit:a", Status::Pass),
                ("kunit:flaky", Status::Pass),
            ]),
        );
        add_run(
            &mut other,
            0,
            &run(&[("boot", Status::Pass), ("kunit:a", Status::Pass)]),
        );
        add_run(&mut other, 1, &run(&[("boot", Status::Pass)]));
        assert_eq!(other["kunit:a"], [Status::Pass, Status::Missing]);
        let units = grade(&reference, &other, 2);
        let by_name: BTreeMap<&str, &Unit> = units.iter().map(|u| (u.name.as_str(), u)).collect();
        assert!(by_name["boot"].graded && by_name["boot"].passed);
        assert!(by_name["kunit:a"].graded && !by_name["kunit:a"].passed);
        assert!(!by_name["kunit:flaky"].graded);
    }

    #[test]
    fn a_racy_suite_is_not_graded() {
        let mut reference = Results::new();
        let mut other = Results::new();
        let units: Units = [
            "kunit:amdv1_iommu_test.test_increase_level",
            "kunit:amdv1_iommu_test.test_increase_level.amdv1_cfg_1",
            "kunit:amdv1_fmt_test.test_attr_from_entry",
            "kunit:snd-soc-cs35l56-test-soundwire.cs35l56_test_parse_xu_onchip_spkid",
            "kunit:amdgpu_dm_hdcp",
            "kunit:amdgpu_dm_hdcp.dm_test_process_output_property_validate_always_scheduled",
            "kunit:stackinit.test_switch_2_none",
            "kunit:stackinit.test_user",
        ]
        .into_iter()
        .map(|u| (u.to_string(), Status::Pass))
        .collect();
        add_run(&mut reference, 0, &units);
        add_run(&mut other, 0, &units);
        let graded: Vec<String> = grade(&reference, &other, 1)
            .into_iter()
            .filter(|u| u.graded)
            .map(|u| u.name)
            .collect();
        assert_eq!(
            graded,
            [
                "kunit:amdv1_fmt_test.test_attr_from_entry",
                "kunit:stackinit.test_user"
            ]
        );
    }

    #[test]
    fn a_selftest_that_measures_time_is_not_graded() {
        let mut reference = Results::new();
        let mut other = Results::new();
        let units: Units = [
            "kselftest:seccomp:seccomp_benchmark",
            "kselftest:seccomp:seccomp_bpf",
            "kselftest:timers:rtcpie",
            "ltp:mm:shm_test01",
        ]
        .into_iter()
        .map(|u| (u.to_string(), Status::Pass))
        .collect();
        add_run(&mut reference, 0, &units);
        add_run(&mut other, 0, &units);
        let graded: Vec<String> = grade(&reference, &other, 1)
            .into_iter()
            .filter(|u| u.graded)
            .map(|u| u.name)
            .collect();
        assert_eq!(graded, ["kselftest:seccomp:seccomp_bpf"]);
    }

    #[test]
    fn failures_are_attributed_by_the_two_other_cells() {
        assert_eq!(attribute(Status::Fail, Status::Pass), "kernel");
        assert_eq!(attribute(Status::Missing, Status::Fail), "kernel");
        assert_eq!(attribute(Status::Pass, Status::Fail), "userland");
        assert_eq!(attribute(Status::Pass, Status::Pass), "interaction");
    }

    #[test]
    fn a_splat_in_some_runs_is_racy_and_one_in_an_inlined_helper_is_the_reference() {
        let set = |list: &[&str]| {
            list.iter()
                .map(|s| (*s).to_string())
                .collect::<BTreeSet<_>>()
        };
        let reference = set(&["BUG: KASAN: slab-out-of-bounds in f_helper+N"]);
        let other = [
            set(&[
                "BUG: KASAN: slab-out-of-bounds in f+N",
                "WARNING: mm/slub.c:1",
            ]),
            set(&[
                "BUG: KASAN: slab-out-of-bounds in f+N",
                "WARNING: mm/slub.c:1",
                "BUG: g+N",
            ]),
        ];
        let splats = sort_splats(reference.clone(), &other);
        assert_eq!(splats.other_only, set(&["WARNING: mm/slub.c:1"]));
        assert_eq!(splats.racy, set(&["BUG: g+N"]));
        let one = sort_splats(reference, &other[1..]);
        assert_eq!(one.other_only, set(&["BUG: g+N", "WARNING: mm/slub.c:1"]));
        assert!(one.racy.is_empty());
    }

    #[test]
    fn a_splat_of_its_own_fails_the_rucc_kernel() {
        let mut o = Outcome {
            reference: PathBuf::new(),
            other: PathBuf::new(),
            kinds: vec![Kind::Boot],
            runs: 1,
            two_userlands: false,
            units: Vec::new(),
            splats: Splats::default(),
        };
        assert!(o.passed());
        o.splats
            .other_only
            .insert("WARNING: CPU: N PID: N at mm/slub.c:1 f+N".to_string());
        assert!(!o.passed());
        assert!(summary(&o).contains("- `WARNING: CPU: N PID: N at mm/slub.c:1 f+N`"));
    }
}
