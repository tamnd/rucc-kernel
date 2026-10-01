//! `rk selftests`: the kernel's selftests built as user programs, for the initramfs (plan 11.5).
//!
//! The collections pinned for a row in `rows.toml` are built from the kernel tree of a build
//! directory, with the compiler asked for and the build's own exported headers, and installed the
//! way `run_kselftest.sh` expects. Each collection is built on its own with `make -k`, so that one
//! program that does not build, often for a library the machine lacks, leaves the rest of its
//! collection standing. The install runs with `make -i` for the same reason. A program listed in
//! `kselftest-list.txt` that is not in the install is recorded as not built, and it stays in the
//! list, so a boot reports it as failed rather than leaving it out.
//!
//! The programs are linked dynamically. Linking them statically breaks real collections (`timers`
//! defines its own `clock_adjtime`, which clashes with the one in `libc.a`), so the initramfs
//! carries the shared libraries each program needs instead, found the way the dynamic linker
//! would find them on the machine that built them.
//!
//! What comes out is the install, `selftests.json` and a build log per collection.

use object::Endianness;
use object::read::elf::{ElfFile64, FileHeader as _, ProgramHeader as _};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the install goes inside the initramfs.
pub const ROOT: &str = "kselftest";

/// One collection's build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Collection {
    /// Its name, the directory under `tools/testing/selftests`.
    pub name: String,
    /// The programs `kselftest-list.txt` names for it.
    pub programs: Vec<String>,
    /// The ones that were not built.
    pub missing: Vec<String>,
}

/// What `rk selftests` did, written to `selftests.json` in the output directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// The kernel tree.
    pub source: PathBuf,
    /// The compiler.
    pub cc: String,
    /// Every collection asked for, in the order asked.
    pub collections: Vec<Collection>,
}

impl Outcome {
    /// Read `selftests.json` from an output directory.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join("selftests.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}; run rk selftests first", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("reading {}: {e}", path.display()))
    }

    /// The names of the collections that have at least one program built.
    #[must_use]
    pub fn runnable(&self) -> Vec<String> {
        self.collections
            .iter()
            .filter(|c| c.missing.len() < c.programs.len())
            .map(|c| c.name.clone())
            .collect()
    }
}

/// Everything `rk selftests` was asked to do.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The kernel tree.
    pub source: PathBuf,
    /// The kernel build directory, whose `usr/include` the programs are built against.
    pub build: PathBuf,
    /// kbuild's `ARCH`.
    pub arch: String,
    /// `CROSS_COMPILE`, empty for a native build.
    pub cross: String,
    /// The compiler, as `CC`.
    pub cc: String,
    /// The collections.
    pub collections: Vec<String>,
    /// Parallel jobs.
    pub jobs: usize,
    /// The output directory.
    pub out: PathBuf,
}

/// The make command in the selftests directory, with what every step shares.
fn make(plan: &Plan, collections: &[String]) -> Command {
    let mut command = Command::new("make");
    command
        .arg("-C")
        .arg(plan.source.join("tools/testing/selftests"))
        .arg(format!("O={}", plan.out.join("build").display()))
        .arg(format!("ARCH={}", plan.arch))
        .arg(format!("CC={}", plan.cc))
        .arg(format!(
            "KHDR_INCLUDES=-isystem {}",
            plan.build.join("usr/include").display()
        ))
        .arg(format!("TARGETS={}", collections.join(" ")));
    if !plan.cross.is_empty() {
        command.arg(format!("CROSS_COMPILE={}", plan.cross));
    }
    command
}

/// Run a command with its output going to a log file. Whether it succeeded is not the question,
/// since `-k` and `-i` keep going: what got installed is.
fn logged(mut command: Command, log: &Path) -> Result<(), String> {
    let file =
        std::fs::File::create(log).map_err(|e| format!("creating {}: {e}", log.display()))?;
    let err = file
        .try_clone()
        .map_err(|e| format!("opening {}: {e}", log.display()))?;
    command
        .stdout(file)
        .stderr(err)
        .status()
        .map_err(|e| format!("running make: {e}"))?;
    Ok(())
}

/// Read `kselftest-list.txt`, `collection:program` per line, into the programs of each.
#[must_use]
pub fn parse_list(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if let Some((collection, program)) = line.split_once(':') {
            out.entry(collection.to_string())
                .or_default()
                .push(program.to_string());
        }
    }
    out
}

/// Export the headers, build every collection, install them, and write `selftests.json`.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    let logs = plan.out.join("logs");
    std::fs::create_dir_all(&logs).map_err(|e| format!("creating {}: {e}", logs.display()))?;
    let mut headers = Command::new("make");
    headers
        .arg("-C")
        .arg(&plan.source)
        .arg(format!("O={}", plan.build.display()))
        .arg(format!("ARCH={}", plan.arch))
        .arg("headers");
    logged(headers, &logs.join("headers.log"))?;
    for collection in &plan.collections {
        eprintln!("rk: selftests {collection}");
        let mut command = make(plan, std::slice::from_ref(collection));
        command.arg("-k").arg(format!("-j{}", plan.jobs));
        logged(command, &logs.join(format!("{collection}.log")))?;
    }
    let install = plan.out.join(ROOT);
    let _ = std::fs::remove_dir_all(&install);
    let mut command = make(plan, &plan.collections);
    command
        .arg("-i")
        .arg(format!("INSTALL_PATH={}", install.display()))
        .arg("install");
    logged(command, &logs.join("install.log"))?;
    let list = std::fs::read_to_string(install.join("kselftest-list.txt")).unwrap_or_default();
    let listed = parse_list(&list);
    let collections = plan
        .collections
        .iter()
        .map(|name| {
            let programs = listed.get(name).cloned().unwrap_or_default();
            let missing = programs
                .iter()
                .filter(|p| !install.join(name).join(p).is_file())
                .cloned()
                .collect();
            Collection {
                name: name.clone(),
                programs,
                missing,
            }
        })
        .collect();
    let outcome = Outcome {
        source: plan.source.clone(),
        cc: plan.cc.clone(),
        collections,
    };
    let json = serde_json::to_string_pretty(&outcome).unwrap_or_default();
    std::fs::write(plan.out.join("selftests.json"), json + "\n")
        .map_err(|e| format!("writing selftests.json: {e}"))?;
    Ok(outcome)
}

/// The outcome as markdown.
#[must_use]
pub fn summary(o: &Outcome) -> String {
    let mut s =
        String::from("### rk selftests\n\n| collection | programs | not built |\n|---|---|---|\n");
    for c in &o.collections {
        let _ = writeln!(
            s,
            "| {} | {} | {} |",
            c.name,
            c.programs.len(),
            if c.missing.is_empty() {
                String::new()
            } else {
                c.missing.join(", ")
            }
        );
    }
    let empty: Vec<&str> = o
        .collections
        .iter()
        .filter(|c| c.programs.is_empty())
        .map(|c| c.name.as_str())
        .collect();
    if !empty.is_empty() {
        let _ = writeln!(
            s,
            "\n{} list no programs, so their directories may not exist in this tree.",
            empty.join(", ")
        );
    }
    s
}

/// The program interpreter and the libraries an ELF file names, or nothing for a file that is
/// not a 64-bit ELF.
fn needs(data: &[u8]) -> (Option<String>, Vec<String>) {
    let Ok(file) = ElfFile64::<Endianness>::parse(data) else {
        return (None, Vec::new());
    };
    let endian = file.endian();
    let interp = file
        .elf_header()
        .program_headers(endian, data)
        .ok()
        .and_then(|headers| {
            headers
                .iter()
                .find_map(|h| h.interpreter(endian, data).ok().flatten())
        })
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
    (interp, needed(data).unwrap_or_default())
}

/// The `DT_NEEDED` names of an ELF file.
fn needed(data: &[u8]) -> Option<Vec<String>> {
    use object::read::elf::{Dyn as _, SectionHeader as _};
    let file = ElfFile64::<Endianness>::parse(data).ok()?;
    let endian = file.endian();
    let sections = file.elf_header().sections(endian, data).ok()?;
    let dynamic = sections
        .iter()
        .find(|s| s.sh_type(endian) == object::elf::SHT_DYNAMIC)?;
    let entries = dynamic.dynamic(endian, data).ok()??.0;
    let strings = sections
        .strings(
            endian,
            data,
            object::SectionIndex(dynamic.sh_link(endian) as usize),
        )
        .ok()?;
    Some(
        entries
            .iter()
            .filter(|d| d.tag32(endian).map(i64::from) == Some(object::elf::DT_NEEDED.0))
            .filter_map(|d| d.string(endian, strings).ok())
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect(),
    )
}

/// Where the dynamic linker would look for a library on this machine.
const LIBRARY_DIRS: &[&str] = &[
    "/lib/x86_64-linux-gnu",
    "/usr/lib/x86_64-linux-gnu",
    "/lib/aarch64-linux-gnu",
    "/usr/lib/aarch64-linux-gnu",
    "/usr/aarch64-linux-gnu/lib",
    "/lib64",
    "/usr/lib64",
    "/lib",
    "/usr/lib",
];

/// A file read by its path inside the initramfs, which is its path on this machine.
fn host_file(path: &str) -> Option<(String, Vec<u8>)> {
    let data = std::fs::read(path).ok()?;
    Some((path.trim_start_matches('/').to_string(), data))
}

/// The interpreter and the shared libraries a set of programs needs, followed through the
/// libraries' own needs, as initramfs files at the paths they have here.
#[must_use]
pub fn libraries<'a>(programs: impl IntoIterator<Item = &'a [u8]>) -> Vec<(String, Vec<u8>)> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    let mut queue: Vec<String> = Vec::new();
    let mut take = |data: &[u8], queue: &mut Vec<String>, out: &mut Vec<(String, Vec<u8>)>| {
        let (interp, libs) = needs(data);
        if let Some(interp) = interp
            && seen.insert(interp.clone())
            && let Some(file) = host_file(&interp)
        {
            out.push(file);
        }
        for lib in libs {
            if seen.insert(lib.clone()) {
                queue.push(lib);
            }
        }
    };
    for data in programs {
        take(data, &mut queue, &mut out);
    }
    while let Some(lib) = queue.pop() {
        let Some(path) = LIBRARY_DIRS
            .iter()
            .map(|dir| format!("{dir}/{lib}"))
            .find(|p| Path::new(p).is_file())
        else {
            continue;
        };
        if let Some(file) = host_file(&path) {
            take(&file.1, &mut queue, &mut out);
            out.push(file);
        }
    }
    out
}

/// Every file under a directory, by its path relative to `base`.
pub fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk(&path, base, out)?;
        } else if path.is_file() {
            let data =
                std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
            let name = path
                .strip_prefix(base)
                .map_err(|e| e.to_string())?
                .display()
                .to_string();
            out.push((name, data));
        }
    }
    Ok(())
}

/// The initramfs files for one collection: the runner, the list, the collection's install and
/// the libraries its programs need.
pub fn files(out: &Path, collection: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    let install = out.join(ROOT);
    let mut files = Vec::new();
    for name in ["run_kselftest.sh", "kselftest-list.txt"] {
        let path = install.join(name);
        let data = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        files.push((format!("{ROOT}/{name}"), data));
    }
    let mut inside = Vec::new();
    walk(&install.join("kselftest"), &install, &mut inside)?;
    let dir = install.join(collection);
    if dir.is_dir() {
        walk(&dir, &install, &mut inside)?;
    }
    let libs = libraries(inside.iter().map(|(_, data)| data.as_slice()));
    files.extend(
        inside
            .into_iter()
            .map(|(name, data)| (format!("{ROOT}/{name}"), data)),
    );
    files.extend(libs);
    Ok(files)
}

/// The results `run_kselftest.sh` printed for one collection, by program, as
/// `kselftest:<collection>:<program>`. The programs' own TAP comes behind `# ` and is not read.
#[must_use]
pub fn units(console: &str) -> BTreeMap<String, crate::tap::Status> {
    let mut out = BTreeMap::new();
    for line in console.lines() {
        let text = crate::boot::strip_timestamp(line.trim_end_matches('\r')).trim();
        let Some((name, status)) = crate::tap::parse_result(text) else {
            continue;
        };
        let Some(rest) = name.strip_prefix("selftests: ") else {
            continue;
        };
        if let Some((collection, program)) = rest.split_once(": ") {
            out.insert(
                format!("kselftest:{}:{}", collection.trim(), program.trim()),
                status,
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tap::Status;

    #[test]
    fn the_list_gives_each_collection_its_programs() {
        let list = parse_list("timers:posix_timers\ntimers:nanosleep\nsize:get_size\n\n");
        assert_eq!(list["timers"], ["posix_timers", "nanosleep"]);
        assert_eq!(list["size"], ["get_size"]);
    }

    #[test]
    fn a_collection_runs_when_one_of_its_programs_was_built() {
        let o = Outcome {
            source: PathBuf::new(),
            cc: "gcc".to_string(),
            collections: vec![
                Collection {
                    name: "timers".to_string(),
                    programs: vec!["a".to_string(), "b".to_string()],
                    missing: vec!["b".to_string()],
                },
                Collection {
                    name: "clone3".to_string(),
                    programs: vec!["clone3".to_string()],
                    missing: vec!["clone3".to_string()],
                },
                Collection {
                    name: "gone".to_string(),
                    programs: Vec::new(),
                    missing: Vec::new(),
                },
            ],
        };
        assert_eq!(o.runnable(), ["timers"]);
        let text = summary(&o);
        assert!(text.contains("| clone3 | 1 | clone3 |"), "{text}");
        assert!(text.contains("gone list no programs"), "{text}");
    }

    #[test]
    fn results_are_read_per_program_and_the_programs_own_tap_is_left_alone() {
        let console = "TAP version 13\n1..3\n# timeout set to 45\n# selftests: timers: posix_timers\n# ok 1 Check itimer virtual... [OK]\n# not ok 2 inner\nok 1 selftests: timers: posix_timers\n[    3.100000] random: crng init done\nnot ok 2 selftests: timers: nanosleep # exit=1\nok 3 selftests: timers: rtcpie # SKIP\n";
        let units = units(console);
        assert_eq!(units.len(), 3, "{units:?}");
        assert_eq!(units["kselftest:timers:posix_timers"], Status::Pass);
        assert_eq!(units["kselftest:timers:nanosleep"], Status::Fail);
        assert_eq!(units["kselftest:timers:rtcpie"], Status::Skip);
    }

    #[test]
    fn a_file_that_is_not_elf_needs_nothing() {
        assert_eq!(needs(b"#!/bin/sh\necho hi\n"), (None, Vec::new()));
        assert!(libraries([&b"#!/bin/sh\n"[..]]).is_empty());
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn a_dynamic_program_brings_its_interpreter_and_libc() {
        let Ok(data) = std::fs::read("/bin/sh") else {
            return;
        };
        let (interp, _) = needs(&data);
        if interp.is_none() {
            return;
        }
        let libs = libraries([data.as_slice()]);
        let names: Vec<&str> = libs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("ld-linux")), "{names:?}");
        assert!(names.iter().any(|n| n.contains("libc.so")), "{names:?}");
    }
}
