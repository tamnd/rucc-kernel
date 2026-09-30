//! `rk boot`: a built kernel started under QEMU with rk-init as PID 1, and what it printed.
//!
//! The initramfs is written here, as a newc archive, rather than by `cpio`, so that the same
//! inputs give the same bytes on any machine. It holds a static busybox, `rk-init/init.sh` as
//! `/init`, and `/dev/console`, which the kernel opens before init runs.
//!
//! The run is read from the serial console. `RK-BOOTED` is the boot marker, each `RK-CHECK` line
//! is one smoke check, and `RK-DONE` means init got to the end. A panic, a timeout or a missing
//! marker is a failed boot. The console goes to `boot.log` and the outcome to `boot.json`.

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
    let entries = [
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

/// A console line without the `[    1.234567] ` printk time in front.
fn strip_timestamp(line: &str) -> &str {
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

/// The QEMU command line.
#[must_use]
pub fn qemu_command(plan: &Plan, image: &Path, kvm: bool) -> Vec<String> {
    let mut words: Vec<String> = [
        plan.row.qemu.as_str(),
        "-M",
        plan.row.machine.as_str(),
        "-m",
        "1G",
        "-smp",
        "2",
        "-nographic",
        "-no-reboot",
        "-accel",
        if kvm { "kvm" } else { "tcg" },
        "-cpu",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    words.push(if kvm && plan.row.arch != "i386" {
        "host".to_string()
    } else {
        plan.row.cpu.clone()
    });
    words.extend([
        "-kernel".to_string(),
        image.display().to_string(),
        "-initrd".to_string(),
        plan.initramfs.display().to_string(),
        "-append".to_string(),
        format!(
            "console={} panic=-1 oops=panic rdinit=/init {}",
            plan.row.console, plan.append
        )
        .trim_end()
        .to_string(),
    ]);
    words
}

/// Boot, and write `boot.log` and `boot.json` in the build directory.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    let image = plan
        .build
        .join("arch")
        .join(srcarch(&plan.row.arch))
        .join("boot")
        .join(&plan.row.image);
    if !image.is_file() {
        return Err(format!("no image at {}; build it first", image.display()));
    }
    let kvm = kvm_usable(&plan.row);
    let command = qemu_command(plan, &image, kvm);
    let mut outcome = Outcome {
        command: command.clone(),
        accel: if kvm { "kvm" } else { "tcg" }.to_string(),
        ..Outcome::default()
    };
    let log_path = plan.build.join("boot.log");
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
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(line) => {
                let _ = writeln!(log, "{line}");
                outcome.read_line(&line);
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
    let _ = child.wait();
    outcome.seconds = clock.elapsed().as_secs_f64();
    let json = serde_json::to_string_pretty(&outcome).unwrap_or_default();
    std::fs::write(plan.build.join("boot.json"), json + "\n")
        .map_err(|e| format!("writing boot.json: {e}"))?;
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
    fn x86_images_live_under_arch_x86() {
        assert_eq!(srcarch("x86_64"), "x86");
        assert_eq!(srcarch("arm64"), "arm64");
    }
}
