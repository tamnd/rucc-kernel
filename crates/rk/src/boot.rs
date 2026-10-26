//! `rk boot`: a built kernel started under QEMU with rk-init as PID 1, and what it printed.
//!
//! The initramfs is written here, as a newc archive, rather than by `cpio`, so that the same
//! inputs give the same bytes on any machine. It holds a static busybox, `rk-init/init.sh` as
//! `/init`, and `/dev/console`, which the kernel opens before init runs.
//!
//! The run is read from the serial console. `RK-BOOTED` is the boot marker, each `RK-CHECK` line
//! is one smoke check, and `RK-DONE` means init got to the end. A panic, a timeout or a missing
//! marker is a failed boot. The console goes to `boot.log` and the outcome to `boot.json`.
//!
//! The kernel can print on the console while init writes a line. Then the line init wrote is cut
//! in two, with kernel lines between the parts. `Mender` puts each such line together again before
//! anything reads it, and `boot.log` keeps the mended lines.

use crate::personas::Row;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The init script, as committed.
pub const INIT: &str = include_str!("../../../rk-init/init.sh");

/// One file in a newc archive.
struct Entry<'a> {
    name: &'a str,
    mode: u32,
    data: &'a [u8],
    rdev: (u32, u32),
}

/// Append one newc header and its name and data, each padded to four bytes.
fn push_entry(out: &mut Vec<u8>, ino: u32, entry: &Entry) {
    let name_size = entry.name.len() + 1;
    let fields = [
        ino,
        entry.mode,
        0,
        0,
        if entry.mode & 0o170_000 == 0o040_000 {
            2
        } else {
            1
        },
        0,
        u32::try_from(entry.data.len()).unwrap_or(u32::MAX),
        0,
        0,
        entry.rdev.0,
        entry.rdev.1,
        u32::try_from(name_size).unwrap_or(u32::MAX),
        0,
    ];
    out.extend_from_slice(b"070701");
    for field in fields {
        out.extend_from_slice(format!("{field:08X}").as_bytes());
    }
    out.extend_from_slice(entry.name.as_bytes());
    out.push(0);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    out.extend_from_slice(entry.data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// The initramfs for a busybox binary, as bytes.
#[must_use]
pub fn initramfs(busybox: &[u8]) -> Vec<u8> {
    initramfs_with(busybox, &[])
}

/// The initramfs for a busybox binary with more files, each given by its path inside and its
/// bytes. The directories they need are made first. An ELF file or a script can be run, and
/// anything else is only read.
#[must_use]
pub fn initramfs_with(busybox: &[u8], files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let base = ["bin", "dev", "proc", "sys", "tmp"];
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (path, _) in files {
        for dir in Path::new(path).ancestors().skip(1) {
            let dir = dir.to_string_lossy();
            if !dir.is_empty() && !base.contains(&dir.as_ref()) {
                dirs.insert(dir.into_owned());
            }
        }
    }
    let mut entries = vec![
        Entry {
            name: "bin",
            mode: 0o040_755,
            data: &[],
            rdev: (0, 0),
        },
        Entry {
            name: "dev",
            mode: 0o040_755,
            data: &[],
            rdev: (0, 0),
        },
        Entry {
            name: "dev/console",
            mode: 0o020_600,
            data: &[],
            rdev: (5, 1),
        },
        Entry {
            name: "proc",
            mode: 0o040_755,
            data: &[],
            rdev: (0, 0),
        },
        Entry {
            name: "sys",
            mode: 0o040_755,
            data: &[],
            rdev: (0, 0),
        },
        Entry {
            name: "tmp",
            mode: 0o041_777,
            data: &[],
            rdev: (0, 0),
        },
        Entry {
            name: "init",
            mode: 0o100_755,
            data: INIT.as_bytes(),
            rdev: (0, 0),
        },
        Entry {
            name: "bin/busybox",
            mode: 0o100_755,
            data: busybox,
            rdev: (0, 0),
        },
    ];
    entries.extend(dirs.iter().map(|dir| Entry {
        name: dir,
        mode: 0o040_755,
        data: &[],
        rdev: (0, 0),
    }));
    entries.extend(files.iter().map(|(path, data)| Entry {
        name: path,
        mode: if data.starts_with(b"\x7fELF") || data.starts_with(b"#!") {
            0o100_755
        } else {
            0o100_644
        },
        data,
        rdev: (0, 0),
    }));
    let mut out = Vec::new();
    for (ino, entry) in (1..).zip(entries.iter()) {
        push_entry(&mut out, ino, entry);
    }
    push_entry(
        &mut out,
        0,
        &Entry {
            name: "TRAILER!!!",
            mode: 0,
            data: &[],
            rdev: (0, 0),
        },
    );
    out
}

/// Whether an ELF file has no program interpreter, which is what a static busybox needs to be.
/// Reads 32 and 64 bit little endian ELF. Anything else is not static as far as this goes.
#[must_use]
pub fn is_static_elf(bytes: &[u8]) -> bool {
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" || bytes[5] != 1 {
        return false;
    }
    let u16_at = |at: usize| usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
    let u32_at = |at: usize| {
        bytes
            .get(at..at + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (phoff, phentsize, phnum) = match bytes[4] {
        1 => (u32_at(0x1C) as usize, u16_at(0x2A), u16_at(0x2C)),
        2 => (
            usize::try_from(u64::from(u32_at(0x20)) | (u64::from(u32_at(0x24)) << 32))
                .unwrap_or(usize::MAX),
            u16_at(0x36),
            u16_at(0x38),
        ),
        _ => return false,
    };
    (0..phnum).all(|i| {
        let at = phoff.saturating_add(i * phentsize);
        at + 4 <= bytes.len() && u32_at(at) != 3
    })
}

/// Whether an ELF program runs on a kernel of that kbuild `ARCH`, read from its `e_machine` and its
/// class. An i386 kernel cannot start a 64 bit program, so a busybox taken from an amd64 runner
/// panics an X32 boot with "No working init found" before any check runs.
#[must_use]
pub fn runs_on(bytes: &[u8], arch: &str) -> bool {
    if bytes.len() < 20 || &bytes[..4] != b"\x7fELF" {
        return false;
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    match arch {
        "x86_64" => machine == 62 || machine == 3,
        "i386" => machine == 3 && bytes[4] == 1,
        "arm64" => machine == 183,
        "riscv" => machine == 243,
        _ => true,
    }
}

/// The most of an initrd QEMU reads, which is what one `read` on Linux gives. A larger one stops
/// QEMU before the guest starts, and a boot that never started tells nothing.
const LARGEST_INITRD: u64 = 0x7fff_f000;

/// Everything `rk boot` was asked to do.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The build directory.
    pub build: PathBuf,
    /// The row it was built for.
    pub row: Row,
    /// The initramfs.
    pub initramfs: PathBuf,
    /// Seconds before the run is stopped and called a timeout.
    pub timeout: u64,
    /// Extra words for the kernel command line.
    pub append: String,
    /// The least memory of the guest in MiB, or 0 for what the initramfs needs.
    pub memory: u64,
    /// Where the console and the outcome go, as `<stem>.log` and `<stem>.json`. By default they
    /// are `boot.log` and `boot.json` in the build directory.
    pub stem: Option<PathBuf>,
}

/// What a boot showed, written to `boot.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// The QEMU command.
    pub command: Vec<String>,
    /// `kvm` or `tcg`.
    pub accel: String,
    /// The kernel release from the marker, if it booted.
    pub booted: Option<String>,
    /// Whether init reached the end.
    pub done: bool,
    /// The smoke checks, by name, with whether each passed.
    pub checks: Vec<(String, bool)>,
    /// The first panic, oops or BUG line, if there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panic: Option<String>,
    /// Whether the run was stopped for taking too long.
    pub timed_out: bool,
    /// Seconds from start to QEMU's exit.
    pub seconds: f64,
}

impl Outcome {
    /// Whether the boot passed: the marker, the end, every check and no panic.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.booted.is_some()
            && self.done
            && self.panic.is_none()
            && !self.checks.is_empty()
            && self.checks.iter().all(|(_, ok)| *ok)
    }

    /// Take one console line into account.
    pub fn read_line(&mut self, line: &str) {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("RK-BOOTED ") {
            self.booted = rest.split_whitespace().next().map(str::to_string);
        } else if let Some(rest) = line.strip_prefix("RK-CHECK ") {
            if let Some((name, result)) = rest.rsplit_once(' ') {
                self.checks.push((name.to_string(), result == "pass"));
            }
        } else if line.starts_with("RK-DONE") {
            self.done = true;
        } else if self.panic.is_none()
            && ["Kernel panic", "Oops:", "BUG:", "general protection fault"]
                .iter()
                .any(|p| line.contains(p))
        {
            self.panic = Some(strip_timestamp(line).to_string());
        }
    }
}

/// Where a printk time such as `[    1.234567]` starts in a line, if the line has one.
fn printk_time(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(open) = line[from..].find('[').map(|i| i + from) {
        let mut i = open + 1;
        while bytes.get(i) == Some(&b' ') {
            i += 1;
        }
        let digits = |mut i: usize| {
            let start = i;
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            (i > start).then_some(i)
        };
        if let Some(dot) = digits(i).filter(|&d| bytes.get(d) == Some(&b'.'))
            && let Some(end) = digits(dot + 1)
            && bytes.get(end) == Some(&b']')
        {
            return Some(open);
        }
        from = open + 1;
    }
    None
}

/// Puts a console line together again when kernel lines cut it.
///
/// A cut line ends in a printk time, as in `RK-CHECK exec pa[    1.238583]  <TASK>`. The kernel
/// line goes on from the time, and the rest of the line init wrote comes at the start of a later
/// line that does not start with a time, as in `ss[    1.238585]  dump_stack_lvl+0x8b/0xf0`. The
/// line is whole when a part of it ends at a newline and not at a time, so the rest can be cut
/// again, and the newline can come alone as an empty line.
#[derive(Debug, Default)]
pub struct Mender {
    pending: Option<String>,
}

impl Mender {
    /// Take one line as the console printed it, and return the whole lines it completes.
    pub fn push(&mut self, line: &str) -> Vec<String> {
        let line = line.trim_end_matches('\r');
        let mut out = Vec::new();
        // init starts every line of its own on a new line. A part waiting when one comes is not
        // init's, as the last line of the firmware before the kernel prints is not.
        if line.starts_with("RK-")
            && let Some(start) = self.pending.take()
        {
            out.push(start);
        }
        match printk_time(line) {
            Some(0) => out.push(line.to_string()),
            Some(cut) => {
                self.pending
                    .get_or_insert_with(String::new)
                    .push_str(&line[..cut]);
                out.push(line[cut..].to_string());
            }
            None => match self.pending.take() {
                Some(mut start) => {
                    start.push_str(line);
                    out.push(start);
                }
                None => out.push(line.to_string()),
            },
        }
        out
    }

    /// The part of a line still waiting for its rest when the console ends.
    pub fn finish(&mut self) -> Option<String> {
        self.pending.take()
    }
}

/// A console line without the `[    1.234567] ` printk time in front.
#[must_use]
pub fn strip_timestamp(line: &str) -> &str {
    line.trim_start()
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
        .map_or(line, |(_, text)| text)
}

/// kbuild's source architecture directory for an `ARCH`.
#[must_use]
pub fn srcarch(arch: &str) -> &str {
    match arch {
        "x86_64" | "i386" => "x86",
        other => other,
    }
}

/// Whether KVM can be used for a row here.
fn kvm_usable(row: &Row) -> bool {
    let native = matches!(
        (std::env::consts::ARCH, row.arch.as_str()),
        ("x86_64", "x86_64" | "i386") | ("aarch64", "arm64") | ("riscv64", "riscv")
    );
    native
        && std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .is_ok()
}

/// The guest's memory in MiB: 1 GiB, and more when the initramfs is big. An LTP runtest file can
/// bring a few hundred megabytes of programs, and the modules of a kunit boot most of a gigabyte.
///
/// The kernel unpacks the archive into a tmpfs, and a tmpfs holds at most half of the memory the
/// kernel counts as its own. While it unpacks, the archive is still reserved and not counted, so
/// half of what is left after the archive and the kernel has to hold the files. With twice the
/// archive and 512 MiB on top, the 509 MB archive of the 7.2.8 kunit boot had about 470 MB of
/// room, and the kernel printed "Initramfs unpacking failed: write error" and ran init with no
/// modules. Three times the archive and 768 MiB leaves a few hundred megabytes over.
#[must_use]
pub fn memory_mib(initramfs: u64) -> u64 {
    let archive = initramfs.div_ceil(1 << 20);
    (768 + 3 * archive).max(1024).next_multiple_of(256)
}

/// The QEMU command line.
#[must_use]
pub fn qemu_command(plan: &Plan, image: &Path, kvm: bool) -> Vec<String> {
    let machine = if plan.row.arch == "arm64" {
        format!("{},gic-version=max", plan.row.machine)
    } else {
        plan.row.machine.clone()
    };
    let initramfs = std::fs::metadata(&plan.initramfs).map_or(0, |m| m.len());
    let memory = format!("{}M", memory_mib(initramfs).max(plan.memory));
    // x86 patches a static key with an INT3 while the other CPUs run, and its handler gives up
    // once the patch is done. With one host thread for each vCPU, QEMU 8.2 can let a vCPU run
    // the INT3 after the patch is done, and the kernel dies with "Oops: int3". The gcc and rucc
    // kernels of 7.2.8 each did this in about one boot of 30. With one thread for both vCPUs
    // every write to the text is seen in order. arm64 patches under stop_machine and needs none.
    let accel = match (kvm, plan.row.arch.as_str()) {
        (true, _) => "kvm",
        (false, "x86_64" | "i386") => "tcg,thread=single",
        (false, _) => "tcg",
    };
    let mut words: Vec<String> = [
        plan.row.qemu.as_str(),
        "-M",
        machine.as_str(),
        "-m",
        memory.as_str(),
        "-smp",
        "2",
        "-nographic",
        "-no-reboot",
        // No network card. Nothing the guest runs uses one, and the default card on an arm64
        // `virt` machine needs an option ROM that a QEMU installed without its recommends lacks.
        "-nic",
        "none",
        "-accel",
        accel,
        "-cpu",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    // The arm64 `max` CPU signs return addresses with QARMA5, which TCG computes in software on
    // every `paciasp` and `autiasp`. A Debian or Fedora kernel signs in most of its functions, and
    // both kernels ran their KUnit boot into rk's 40 minute limit. QEMU's own docs point at
    // `pauth-impdef=on` for TCG, a cheap hash in place of QARMA5. The kernel finds address
    // authentication either way and still signs and checks every return.
    words.push(if kvm && plan.row.arch != "i386" {
        "host".to_string()
    } else if plan.row.arch == "arm64" && plan.row.cpu == "max" {
        "max,pauth-impdef=on".to_string()
    } else {
        plan.row.cpu.clone()
    });
    // A kernel that dies before its serial driver is up prints nothing on the console, and with
    // `panic=-1` and `-no-reboot` QEMU just exits. On a machine with a device tree a bare
    // `earlycon` finds the UART from `/chosen` and the oops is in the log.
    let early = if matches!(plan.row.arch.as_str(), "arm64" | "riscv") {
        "earlycon "
    } else {
        ""
    };
    // On the 7.3 arm64 nightly the kernel set memory with FEAT_MOPS, and in a KUnit boot one module
    // load in a few hundred stopped in `__memset` in `module_memory_alloc`. The SETM wrote the
    // page after the end of the area with no bytes left to set, and the kernel took a paging
    // fault there. The gcc kernel with its own modules did it too, so the fault is not the
    // compiler's, and QEMU 8.2's MOPS under TCG is the likely cause. With `arm64.nomops` the
    // kernel uses its own copy and set loops on both kernels. Under KVM the CPU runs the
    // instructions itself.
    let mops = if plan.row.arch == "arm64" && !kvm {
        "arm64.nomops "
    } else {
        ""
    };
    // In 7.2 the clocksource watchdog checks the TSC of each CPU from a timer. It queues a call
    // on the other CPU and waits for it with interrupts off. If the timer interrupts a CPU that
    // has queued a call on the same CPU but not yet sent the IPI, the IPI never goes out. The
    // wait has a timeout, but early in boot the clock it reads is jiffies, and jiffies stop
    // while the waiting CPU has interrupts off. The boot hangs in SMP bringup. The rucc kernel
    // of 7.2.8 did this once in about 60 boots, and the gcc kernel has the same window. With
    // `tsc=nowatchdog` the TSC is not watched and the watchdog does not run.
    //
    // At boot x86 also checks that the IO-APIC timer ticks within a short delay that it measures
    // with the TSC. Under TCG on a loaded host the vCPU can stop for longer than the delay, and
    // the kernel panics with "IO-APIC + timer doesn't work". The gcc and rucc kernels both did
    // this in about one boot of 3 when the host load was high. `no_timer_check` skips the
    // check, as virtual machines usually do.
    let tsc = if matches!(plan.row.arch.as_str(), "x86_64" | "i386") {
        "tsc=nowatchdog no_timer_check "
    } else {
        ""
    };
    // A config can build KUnit in and leave it off until `kunit.enable=1`, as Fedora's does with
    // KUNIT_DEFAULT_ENABLED unset. The modules load and say "kunit: disabled", and no suite runs.
    //
    // The KUnit suites write about 15 MB to the log, faster than the serial console prints it.
    // When the log buffer is full, the kernel overwrites lines that the console did not print,
    // and it prints "** N printk messages dropped **" in their place. Each kernel then loses
    // different units, and rk sees them as missing. A 32 MB buffer holds the full output.
    //
    // KUnit gives a case 300 seconds, and a slow one three times that, and then runs the exit
    // while the case is still going. Under TCG the host decides how long a case takes. On the
    // 7.2.8 arm64 nightly `binder_alloc_exhaustive_test` took 622 seconds on the reference and
    // 891 on the rucc kernel with the reference modules, where its exit then found the mapping
    // still up and hit a `BUG_ON`. `kunit.timeout=900` gives a slow case 45 minutes. 6.12.111
    // has a fixed 300 seconds and no such parameter, and says it is unknown and goes on.
    let kunit = if plan
        .append
        .split_whitespace()
        .any(|w| w == "rk.suite=kunit")
    {
        "kunit.enable=1 kunit.timeout=900 log_buf_len=32M "
    } else {
        ""
    };
    words.extend([
        "-kernel".to_string(),
        image.display().to_string(),
        "-initrd".to_string(),
        plan.initramfs.display().to_string(),
        "-append".to_string(),
        format!(
            "console={} {early}{mops}{tsc}{kunit}panic=-1 oops=panic rdinit=/init {}",
            plan.row.console, plan.append
        )
        .trim_end()
        .to_string(),
    ]);
    words
}

/// Boot, and write `boot.log` and `boot.json` in the build directory.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    let image = crate::build::image_path(&plan.build, &plan.row);
    if !image.is_file() {
        return Err(format!("no image at {}; build it first", image.display()));
    }
    let size = std::fs::metadata(&plan.initramfs).map_or(0, |m| m.len());
    if size > LARGEST_INITRD {
        return Err(format!(
            "{} is {size} bytes, more than the {LARGEST_INITRD} QEMU reads of an initrd",
            plan.initramfs.display()
        ));
    }
    let kvm = kvm_usable(&plan.row);
    let command = qemu_command(plan, &image, kvm);
    let mut outcome = Outcome {
        command: command.clone(),
        accel: if kvm { "kvm" } else { "tcg" }.to_string(),
        ..Outcome::default()
    };
    let stem = plan.stem.clone().unwrap_or_else(|| plan.build.join("boot"));
    let log_path = stem.with_extension("log");
    let mut log = std::fs::File::create(&log_path)
        .map_err(|e| format!("creating {}: {e}", log_path.display()))?;
    let clock = Instant::now();
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("running {}: {e}", command[0]))?;
    let (send, lines) = mpsc::channel();
    let stdout = child.stdout.take().ok_or("no stdout from QEMU")?;
    let stderr = child.stderr.take().ok_or("no stderr from QEMU")?;
    let err_send = send.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = send.send(line);
        }
    });
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = err_send.send(format!("qemu: {line}"));
        }
    });
    let deadline = clock + Duration::from_secs(plan.timeout);
    let mut unpacked = None;
    let mut mender = Mender::default();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(line) => {
                for line in mender.push(&line) {
                    let _ = writeln!(log, "{line}");
                    if unpacked.is_none() && line.contains("Initramfs unpacking failed") {
                        unpacked = Some(strip_timestamp(&line).to_string());
                    }
                    outcome.read_line(&line);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    outcome.timed_out = true;
                    let _ = child.kill();
                    break;
                }
            }
        }
    }
    if let Some(line) = mender.finish() {
        let _ = writeln!(log, "{line}");
        outcome.read_line(&line);
    }
    let _ = child.wait();
    outcome.seconds = clock.elapsed().as_secs_f64();
    let json = serde_json::to_string_pretty(&outcome).unwrap_or_default();
    let json_path = stem.with_extension("json");
    std::fs::write(&json_path, json + "\n")
        .map_err(|e| format!("writing {}: {e}", json_path.display()))?;
    // The kernel goes on to run init from what it did unpack, so the boot looks like one that had
    // nothing to do. A kunit boot whose modules were cut off loaded none and reported none.
    if let Some(line) = unpacked {
        return Err(format!(
            "{} did not unpack in the guest, \"{line}\"; see {}",
            plan.initramfs.display(),
            log_path.display()
        ));
    }
    Ok(outcome)
}

/// The boot as markdown.
#[must_use]
pub fn summary(o: &Outcome) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Boot under {}: {} in {:.0} seconds.",
        o.accel,
        if o.passed() { "passed" } else { "failed" },
        o.seconds
    );
    let _ = writeln!(s, "\n| | |\n|---|---|");
    let _ = writeln!(s, "| booted | {} |", o.booted.as_deref().unwrap_or("no"));
    let _ = writeln!(
        s,
        "| init finished | {} |",
        if o.done { "yes" } else { "no" }
    );
    if o.timed_out {
        let _ = writeln!(s, "| timed out | yes |");
    }
    if let Some(panic) = &o.panic {
        let _ = writeln!(s, "| panic | `{}` |", panic.replace('|', "\\|"));
    }
    for (name, ok) in &o.checks {
        let _ = writeln!(s, "| {name} | {} |", if *ok { "pass" } else { "fail" });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_is_newc_with_a_trailer() {
        let archive = initramfs(b"\x7fELF");
        assert!(archive.starts_with(b"070701"));
        assert_eq!(archive.len() % 4, 0);
        let text = String::from_utf8_lossy(&archive);
        assert!(text.contains("dev/console"));
        assert!(text.contains("#!/bin/busybox sh"));
        assert!(text.ends_with("TRAILER!!!\0\0\0\0"));
        assert_eq!(initramfs(b"x"), initramfs(b"x"));
    }

    #[test]
    fn extra_files_come_with_their_directories() {
        let archive = initramfs_with(
            b"x",
            &[("lib/modules/rk/kernel/a.ko".to_string(), b"ko".to_vec())],
        );
        let text = String::from_utf8_lossy(&archive);
        let lib = text.find("lib\0").unwrap();
        let rk = text.find("lib/modules/rk\0").unwrap();
        let ko = text.find("lib/modules/rk/kernel/a.ko\0").unwrap();
        assert!(lib < rk && rk < ko);
        assert_eq!(text.matches("bin\0").count(), 1);
    }

    #[test]
    fn an_interpreter_means_not_static() {
        let mut elf = vec![0u8; 0x100];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[0x20] = 0x40;
        elf[0x36] = 0x38;
        elf[0x38] = 1;
        elf[0x40] = 1;
        assert!(is_static_elf(&elf));
        elf[0x40] = 3;
        assert!(!is_static_elf(&elf));
        assert!(!is_static_elf(b"#!/bin/sh"));
    }

    #[test]
    fn a_busybox_is_checked_against_the_row_it_boots() {
        let mut elf = vec![0u8; 0x40];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[18] = 62;
        assert!(runs_on(&elf, "x86_64"));
        assert!(!runs_on(&elf, "i386"));
        assert!(!runs_on(&elf, "arm64"));
        elf[4] = 1;
        elf[18] = 3;
        assert!(runs_on(&elf, "i386"));
        assert!(runs_on(&elf, "x86_64"));
        elf[4] = 2;
        elf[18] = 183;
        assert!(runs_on(&elf, "arm64"));
        assert!(!runs_on(b"#!/bin/sh", "x86_64"));
    }

    #[test]
    fn the_console_is_read_for_markers_checks_and_panics() {
        let mut o = Outcome::default();
        for line in [
            "[    0.000000] Linux version 7.2.8",
            "RK-BOOTED 7.2.8 1.23\r",
            "RK-CHECK fork pass",
            "RK-CHECK tmp-write fail",
            "[    2.100000] Kernel panic - not syncing: Attempted to kill init!",
            "RK-DONE",
        ] {
            o.read_line(line);
        }
        assert_eq!(o.booted.as_deref(), Some("7.2.8"));
        assert_eq!(
            o.checks,
            [("fork".to_string(), true), ("tmp-write".to_string(), false)]
        );
        assert_eq!(
            o.panic.as_deref(),
            Some("Kernel panic - not syncing: Attempted to kill init!")
        );
        assert!(o.done);
        assert!(!o.passed());
    }

    #[test]
    fn a_line_the_kernel_cut_is_put_together_again() {
        // From the console of a 7.3 defconfig boot, where KUnit printed stack dumps while init
        // ran the smoke checks.
        let console = [
            "Booting from ROM..[    0.000000] Linux version 7.3.0",
            "RK-BOOTED 7.3.0 [    1.238260]  </TASK>",
            "1.10[    1.238262] kobject: kobject_add_internal failed",
            "",
            "[    1.238582] Call Trace:",
            "RK-CHECK exec pa[    1.238583]  <TASK>",
            "ss[    1.238585]  dump_stack_lvl+0x8b/0xf0",
            "[    1.238590]  sysfs_warn_dup.cold+0x19/0x26",
            "",
            "RK-CHECK pipe pa[    1.238703]  ret_from_fork_asm+0x1a/0x30",
            "[    1.238708]  </TASK>",
            "ss",
            "RK-CHECK signal [    1.239048] Call Trace:",
            "pass",
            "RK-CHECK cpus pass",
            "RK-DONE",
        ];
        let mut mender = Mender::default();
        let lines: Vec<String> = console.iter().flat_map(|l| mender.push(l)).collect();
        assert_eq!(mender.finish(), None);
        assert_eq!(
            lines,
            [
                "[    0.000000] Linux version 7.3.0",
                "Booting from ROM..",
                "[    1.238260]  </TASK>",
                "[    1.238262] kobject: kobject_add_internal failed",
                "RK-BOOTED 7.3.0 1.10",
                "[    1.238582] Call Trace:",
                "[    1.238583]  <TASK>",
                "[    1.238585]  dump_stack_lvl+0x8b/0xf0",
                "[    1.238590]  sysfs_warn_dup.cold+0x19/0x26",
                "RK-CHECK exec pass",
                "[    1.238703]  ret_from_fork_asm+0x1a/0x30",
                "[    1.238708]  </TASK>",
                "RK-CHECK pipe pass",
                "[    1.239048] Call Trace:",
                "RK-CHECK signal pass",
                "RK-CHECK cpus pass",
                "RK-DONE",
            ]
        );
        let mut o = Outcome::default();
        for line in &lines {
            o.read_line(line);
        }
        assert_eq!(o.booted.as_deref(), Some("7.3.0"));
        assert_eq!(o.checks.len(), 4);
        assert!(o.passed());
    }

    #[test]
    fn a_printk_time_is_found_only_where_it_is() {
        assert_eq!(printk_time("[    1.238583]  <TASK>"), Some(0));
        assert_eq!(printk_time("ss[ 12.5] x"), Some(2));
        assert_eq!(printk_time("RK-CHECK [x] pass"), None);
        assert_eq!(printk_time("a[1] b[ 2.] c"), None);
        assert_eq!(printk_time("RK-CHECK fork pass"), None);
    }

    #[test]
    fn a_big_initramfs_gets_more_memory() {
        assert_eq!(memory_mib(0), 1024);
        assert_eq!(memory_mib(80 << 20), 1024);
        assert_eq!(memory_mib(400 << 20), 2048);
        assert_eq!(memory_mib((400 << 20) + 1), 2048);
        assert_eq!(memory_mib(975 << 20), 3840);
    }

    #[test]
    fn a_device_tree_machine_gets_an_early_console() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rows.toml");
        let rows = crate::personas::Rows::load(&path).unwrap();
        let line = |name: &str| {
            let plan = Plan {
                build: PathBuf::from("b"),
                row: rows.get(name).unwrap().clone(),
                initramfs: PathBuf::from("i"),
                timeout: 1,
                append: String::new(),
                memory: 0,
                stem: None,
            };
            qemu_command(&plan, Path::new("Image"), false)
                .pop()
                .unwrap()
        };
        assert_eq!(
            line("A64"),
            "console=ttyAMA0 earlycon arm64.nomops panic=-1 oops=panic rdinit=/init"
        );
        assert_eq!(
            line("X64"),
            "console=ttyS0 tsc=nowatchdog no_timer_check panic=-1 oops=panic rdinit=/init"
        );
    }

    #[test]
    fn a_kunit_boot_turns_kunit_on() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rows.toml");
        let rows = crate::personas::Rows::load(&path).unwrap();
        let line = |append: &str| {
            let plan = Plan {
                build: PathBuf::from("b"),
                row: rows.get("A64").unwrap().clone(),
                initramfs: PathBuf::from("i"),
                timeout: 1,
                append: append.to_string(),
                memory: 0,
                stem: None,
            };
            qemu_command(&plan, Path::new("Image"), false)
                .pop()
                .unwrap()
        };
        assert!(
            line("rk.suite=kunit").contains(" kunit.enable=1 kunit.timeout=900 log_buf_len=32M ")
        );
        assert!(!line("rk.suite=smoke").contains("kunit.enable"));
    }

    #[test]
    fn x86_under_tcg_runs_both_cpus_on_one_thread() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rows.toml");
        let rows = crate::personas::Rows::load(&path).unwrap();
        let accel = |name: &str, kvm: bool| {
            let plan = Plan {
                build: PathBuf::from("b"),
                row: rows.get(name).unwrap().clone(),
                initramfs: PathBuf::from("i"),
                timeout: 1,
                append: String::new(),
                memory: 0,
                stem: None,
            };
            let words = qemu_command(&plan, Path::new("Image"), kvm);
            let at = words.iter().position(|w| w == "-accel").unwrap();
            words[at + 1].clone()
        };
        assert_eq!(accel("X64", false), "tcg,thread=single");
        assert_eq!(accel("X32", false), "tcg,thread=single");
        assert_eq!(accel("A64", false), "tcg");
        assert_eq!(accel("X64", true), "kvm");
    }

    #[test]
    fn x86_images_live_under_arch_x86() {
        assert_eq!(srcarch("x86_64"), "x86");
        assert_eq!(srcarch("arm64"), "arm64");
    }
}
