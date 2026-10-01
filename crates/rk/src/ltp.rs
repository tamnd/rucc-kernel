//! `rk ltp`: the Linux Test Project built as user programs, for the initramfs (plan 11.5).
//!
//! The release pinned in `ltp.toml` is downloaded into the cache, checked against its hash and
//! unpacked once. `rk ltp` copies the tree into its output directory and configures it with the
//! compiler asked for. LTP's kernel modules are left out, since they would be built for this
//! machine's kernel rather than the one under test. The build runs with `make -k`, so that a test
//! that does not build leaves the rest standing, and the install with `make -i`, under `/ltp`,
//! the way it sits in the initramfs.
//!
//! A runtest file is a list of tests, a tag and a command per line. The runtest files pinned for
//! the row in `rows.toml` are each one boot of `rk test --kinds ltp`, and every tag in them is a
//! unit, `ltp:<runtest>:<tag>`. A tag whose program was not installed stays in the file, so a boot
//! reports it as failed rather than leaving it out, and `ltp.json` lists it as not built.
//!
//! The initramfs carries the runtest file, the programs its commands name, the shell scripts and
//! `tst_` helpers the tests share, the data of the tests it runs and the shared libraries all of
//! them need. The whole install does not fit next to a kernel in the guest's memory.
//!
//! What comes out is the install, `ltp.json` and the configure, build and install logs.

use crate::selftests::libraries;
use rk_shim::digest::sha256_file;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the install goes inside the initramfs, and the prefix it is configured for.
pub const ROOT: &str = "ltp";

/// The file in an unpacked tree that holds the hash of the tarball it came from.
const MARKER: &str = ".rk-sha256";

/// The pinned release, `ltp.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pin {
    /// The release, as in `20260529`.
    pub version: String,
    /// The tarball.
    pub url: String,
    /// The tarball's SHA-256.
    pub sha256: String,
}

impl Pin {
    /// Read `ltp.toml`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let pin: Self = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if pin.sha256.len() != 64 {
            return Err(format!("{}: sha256 is not a SHA-256", path.display()));
        }
        Ok(pin)
    }

    /// The unpacked tree in the cache, downloading and unpacking the tarball when the tree is not
    /// there yet.
    pub fn fetch(&self, cache: &Path) -> Result<PathBuf, String> {
        let dir = cache.join("ltp");
        let tree = dir.join(format!("ltp-full-{}", self.version));
        if std::fs::read_to_string(tree.join(MARKER)).is_ok_and(|h| h.trim() == self.sha256) {
            return Ok(tree);
        }
        let archive = dir.join(format!("ltp-full-{}.tar.xz", self.version));
        if !archive.is_file() {
            crate::kernelorg::download(&self.url, &archive)?;
        }
        let got =
            sha256_file(&archive).map_err(|e| format!("hashing {}: {e}", archive.display()))?;
        if got != self.sha256 {
            let _ = std::fs::remove_file(&archive);
            return Err(format!(
                "{} has SHA-256 {got}, and ltp.toml says {}; the file was removed",
                archive.display(),
                self.sha256
            ));
        }
        if tree.exists() {
            std::fs::remove_dir_all(&tree)
                .map_err(|e| format!("removing {}: {e}", tree.display()))?;
        }
        let status = Command::new("tar")
            .arg("-xJf")
            .arg(&archive)
            .arg("-C")
            .arg(&dir)
            .status()
            .map_err(|e| format!("running tar: {e}"))?;
        if !status.success() || !tree.join("configure").is_file() {
            return Err(format!(
                "unpacking {} did not give {}",
                archive.display(),
                tree.display()
            ));
        }
        std::fs::write(tree.join(MARKER), &self.sha256)
            .map_err(|e| format!("writing the marker in {}: {e}", tree.display()))?;
        Ok(tree)
    }
}

/// One test of a runtest file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Test {
    /// The tag, which names the unit.
    pub tag: String,
    /// The command, run with `sh -c`.
    pub command: String,
}

/// Read a runtest file. Blank lines and comments are not tests.
#[must_use]
pub fn parse_runtest(text: &str) -> Vec<Test> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (tag, command) = line.split_once(char::is_whitespace)?;
            Some(Test {
                tag: tag.to_string(),
                command: command.trim().to_string(),
            })
        })
        .collect()
}

/// The words of a command that may name a program, without the quotes and operators of the
/// shell around them.
fn words(command: &str) -> impl Iterator<Item = &str> {
    command
        .split(|c: char| c.is_whitespace() || ";|&()<>\"'`".contains(c))
        .filter(|w| !w.is_empty() && !w.starts_with('-') && !w.contains('/') && !w.contains('$'))
}

/// One runtest file's build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Runtest {
    /// Its name, the file under `runtest/`.
    pub name: String,
    /// The tags of its tests.
    pub tests: Vec<String>,
    /// The tags whose program was not installed.
    pub missing: Vec<String>,
}

/// What `rk ltp` did, written to `ltp.json` in the output directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// The release.
    pub version: String,
    /// The compiler.
    pub cc: String,
    /// Every runtest file asked for, in the order asked.
    pub runtests: Vec<Runtest>,
}

impl Outcome {
    /// Read `ltp.json` from an output directory.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join("ltp.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}; run rk ltp first", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("reading {}: {e}", path.display()))
    }

    /// The names of the runtest files that have at least one test built.
    #[must_use]
    pub fn runnable(&self) -> Vec<String> {
        self.runtests
            .iter()
            .filter(|r| r.missing.len() < r.tests.len())
            .map(|r| r.name.clone())
            .collect()
    }
}

/// Everything `rk ltp` was asked to do.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The unpacked release.
    pub source: PathBuf,
    /// The release.
    pub version: String,
    /// The GNU triplet to configure for, empty for a native build.
    pub host: String,
    /// The `strip` for the programs, which keep a static copy of LTP's library and its debug
    /// information each.
    pub strip: String,
    /// The compiler, as `CC`.
    pub cc: String,
    /// The runtest files.
    pub runtests: Vec<String>,
    /// Parallel jobs.
    pub jobs: usize,
    /// The output directory.
    pub out: PathBuf,
}

/// Run a command in a directory with its output going to a log file, and say whether it
/// succeeded.
fn logged(mut command: Command, dir: &Path, log: &Path) -> Result<bool, String> {
    let file =
        std::fs::File::create(log).map_err(|e| format!("creating {}: {e}", log.display()))?;
    let err = file
        .try_clone()
        .map_err(|e| format!("opening {}: {e}", log.display()))?;
    let status = command
        .current_dir(dir)
        .stdout(file)
        .stderr(err)
        .status()
        .map_err(|e| format!("running {}: {e}", command.get_program().display()))?;
    Ok(status.success())
}

/// Whether a test's program is in the install. A command that names no installed program at all
/// is a test that was not built.
fn built(test: &Test, bin: &Path) -> bool {
    words(&test.command).any(|w| bin.join(w).is_file())
}

/// Whether a file starts like an ELF file.
fn is_elf(path: &Path) -> bool {
    use std::io::Read as _;
    let mut magic = [0u8; 4];
    std::fs::File::open(path).is_ok_and(|mut f| f.read_exact(&mut magic).is_ok())
        && magic == *b"\x7fELF"
}

/// Take the debug information out of every program in a directory, which leaves about a third of
/// each. A program `strip` cannot read is left as it is.
fn strip_debug(strip: &str, bin: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(bin).map_err(|e| format!("reading {}: {e}", bin.display()))?;
    let programs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| is_elf(p))
        .collect();
    for chunk in programs.chunks(200) {
        Command::new(strip)
            .arg("--strip-debug")
            .args(chunk)
            .status()
            .map_err(|e| format!("running {strip}: {e}"))?;
    }
    Ok(())
}

/// Seconds a boot running one runtest file may take: `base`, and three more for each test in it,
/// since a file holds anything from one test to fifteen hundred.
#[must_use]
pub fn timeout(base: u64, files: &[(String, Vec<u8>)]) -> u64 {
    let tests = files
        .iter()
        .find(|(name, _)| name.starts_with(&format!("{ROOT}/runtest/")))
        .map_or(0, |(_, data)| {
            parse_runtest(&String::from_utf8_lossy(data)).len()
        });
    base + 3 * u64::try_from(tests).unwrap_or(u64::MAX / 4)
}

/// Copy the tree, configure, build and install it, and write `ltp.json`.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    let logs = plan.out.join("logs");
    std::fs::create_dir_all(&logs).map_err(|e| format!("creating {}: {e}", logs.display()))?;
    let build = plan.out.join("build");
    let _ = std::fs::remove_dir_all(&build);
    let status = Command::new("cp")
        .arg("-R")
        .arg(&plan.source)
        .arg(&build)
        .status()
        .map_err(|e| format!("running cp: {e}"))?;
    if !status.success() {
        return Err(format!("copying {} failed", plan.source.display()));
    }
    let mut configure = Command::new("./configure");
    configure
        .arg(format!("--prefix=/{ROOT}"))
        .arg("--without-modules")
        .arg(format!("CC={}", plan.cc));
    if !plan.host.is_empty() {
        configure.arg(format!("--host={}", plan.host));
    }
    eprintln!("rk: ltp configure");
    if !logged(configure, &build, &logs.join("configure.log"))? {
        return Err(format!(
            "configuring LTP with {} failed; see {}",
            plan.cc,
            logs.join("configure.log").display()
        ));
    }
    eprintln!("rk: ltp build");
    let mut make = Command::new("make");
    make.arg("-k").arg(format!("-j{}", plan.jobs));
    logged(make, &build, &logs.join("build.log"))?;
    let install = plan.out.join("install");
    let _ = std::fs::remove_dir_all(&install);
    let mut make = Command::new("make");
    make.arg("-i")
        .arg(format!("DESTDIR={}", install.display()))
        .arg("install");
    logged(make, &build, &logs.join("install.log"))?;
    let root = install.join(ROOT);
    let bin = root.join("testcases/bin");
    strip_debug(&plan.strip, &bin)?;
    let mut runtests = Vec::new();
    for name in &plan.runtests {
        let text = std::fs::read_to_string(root.join("runtest").join(name)).unwrap_or_default();
        let tests = parse_runtest(&text);
        runtests.push(Runtest {
            name: name.clone(),
            tests: tests.iter().map(|t| t.tag.clone()).collect(),
            missing: tests
                .iter()
                .filter(|t| !built(t, &bin))
                .map(|t| t.tag.clone())
                .collect(),
        });
    }
    let outcome = Outcome {
        version: plan.version.clone(),
        cc: plan.cc.clone(),
        runtests,
    };
    let json = serde_json::to_string_pretty(&outcome).unwrap_or_default();
    std::fs::write(plan.out.join("ltp.json"), json + "\n")
        .map_err(|e| format!("writing ltp.json: {e}"))?;
    Ok(outcome)
}

/// The outcome as markdown.
#[must_use]
pub fn summary(o: &Outcome) -> String {
    let mut s = format!(
        "### rk ltp {}\n\n| runtest | tests | not built |\n|---|---|---|\n",
        o.version
    );
    for r in &o.runtests {
        let _ = writeln!(
            s,
            "| {} | {} | {} |",
            r.name,
            r.tests.len(),
            r.missing.len()
        );
    }
    let empty: Vec<&str> = o
        .runtests
        .iter()
        .filter(|r| r.tests.is_empty())
        .map(|r| r.name.as_str())
        .collect();
    if !empty.is_empty() {
        let _ = writeln!(
            s,
            "\n{} list no tests, so this release may not have them.",
            empty.join(", ")
        );
    }
    s
}

/// Whether a file in `testcases/bin` goes in every LTP initramfs: the shell libraries and
/// scripts, which are small, and the `tst_` helpers they call.
fn shared(name: &str, data: &[u8]) -> bool {
    name.starts_with("tst_") || !data.starts_with(b"\x7fELF")
}

/// The initramfs files for one runtest file: the file, the programs its commands name, the shared
/// scripts and helpers, the data of its tests and the libraries the programs need.
pub fn files(out: &Path, runtest: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    let root = out.join("install").join(ROOT);
    let path = root.join("runtest").join(runtest);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let named: BTreeSet<String> = parse_runtest(&text)
        .iter()
        .flat_map(|t| words(&t.command).map(str::to_string).collect::<Vec<_>>())
        .collect();
    let mut files = vec![(
        format!("{ROOT}/runtest/{runtest}"),
        text.clone().into_bytes(),
    )];
    let bin = root.join("testcases/bin");
    let entries = std::fs::read_dir(&bin).map_err(|e| format!("reading {}: {e}", bin.display()))?;
    let mut programs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().is_file() {
            continue;
        }
        let data = std::fs::read(entry.path())
            .map_err(|e| format!("reading {}: {e}", entry.path().display()))?;
        if named.contains(&name) || shared(&name, &data) {
            programs.insert(name, data);
        }
    }
    let data = root.join("testcases/data");
    for name in programs.keys() {
        let dir = data.join(name);
        if dir.is_dir() {
            let mut inside = Vec::new();
            crate::selftests::walk(&dir, &root, &mut inside)?;
            files.extend(
                inside
                    .into_iter()
                    .map(|(name, data)| (format!("{ROOT}/{name}"), data)),
            );
        }
    }
    let libs = libraries(programs.values().map(Vec::as_slice));
    files.extend(
        programs
            .into_iter()
            .map(|(name, data)| (format!("{ROOT}/testcases/bin/{name}"), data)),
    );
    files.extend(libs);
    Ok(files)
}

/// The results `rk-init` printed for one runtest file, as `ltp:<runtest>:<tag>`. A test that ran
/// into nothing to test on this machine, LTP's exit code 32, is a skip.
#[must_use]
pub fn units(console: &str) -> BTreeMap<String, crate::tap::Status> {
    use crate::tap::Status;
    let mut out = BTreeMap::new();
    for line in console.lines() {
        let text = crate::boot::strip_timestamp(line.trim_end_matches('\r')).trim();
        let Some(rest) = text.strip_prefix("RK-LTP ") else {
            continue;
        };
        let mut words = rest.split_whitespace();
        let (Some(runtest), Some(tag), Some(result)) = (words.next(), words.next(), words.next())
        else {
            continue;
        };
        let status = match result {
            "pass" => Status::Pass,
            "skip" => Status::Skip,
            _ => Status::Fail,
        };
        out.insert(format!("ltp:{runtest}:{tag}"), status);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tap::Status;

    #[test]
    fn a_runtest_file_reads_as_tags_and_commands() {
        let tests = parse_runtest(
            "#DESCRIPTION:Kernel system calls\nabort01 abort01\n\naccess01  access01 -i 2\n  # a comment\nbad\n",
        );
        assert_eq!(
            tests,
            [
                Test {
                    tag: "abort01".to_string(),
                    command: "abort01".to_string()
                },
                Test {
                    tag: "access01".to_string(),
                    command: "access01 -i 2".to_string()
                },
            ]
        );
    }

    #[test]
    fn a_command_names_its_programs() {
        let named: Vec<&str> =
            words("export TCbin=$LTPROOT/testcases/bin; fsx-linux -N 10000 -o 8192 \"a b\"")
                .collect();
        assert!(named.contains(&"fsx-linux"), "{named:?}");
        assert!(named.contains(&"export"), "{named:?}");
        assert!(!named.iter().any(|w| w.contains('/') || w.starts_with('-')));
    }

    #[test]
    fn a_test_is_built_when_its_program_was_installed() {
        let dir = std::env::temp_dir().join(format!("rk-ltp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("abort01"), b"\x7fELF").unwrap();
        let test = |command: &str| Test {
            tag: "t".to_string(),
            command: command.to_string(),
        };
        assert!(built(&test("abort01"), &dir));
        assert!(built(&test("sh -c \"abort01 -i 2\""), &dir));
        assert!(!built(&test("access01"), &dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn results_are_read_per_tag() {
        let console = "[    2.000000] RK-LTP syscalls abort01 pass 0\n# abort01.c:52: TFAIL: oops\nRK-LTP syscalls access01 fail 1\nRK-LTP syscalls acct02 skip 32\nRK-LTP syscalls\n";
        let units = units(console);
        assert_eq!(units.len(), 3, "{units:?}");
        assert_eq!(units["ltp:syscalls:abort01"], Status::Pass);
        assert_eq!(units["ltp:syscalls:access01"], Status::Fail);
        assert_eq!(units["ltp:syscalls:acct02"], Status::Skip);
    }

    #[test]
    fn a_runtest_file_runs_when_one_of_its_tests_was_built() {
        let o = Outcome {
            version: "20260529".to_string(),
            cc: "gcc".to_string(),
            runtests: vec![
                Runtest {
                    name: "syscalls".to_string(),
                    tests: vec!["a".to_string(), "b".to_string()],
                    missing: vec!["b".to_string()],
                },
                Runtest {
                    name: "gone".to_string(),
                    tests: Vec::new(),
                    missing: Vec::new(),
                },
            ],
        };
        assert_eq!(o.runnable(), ["syscalls"]);
        let text = summary(&o);
        assert!(text.contains("| syscalls | 2 | 1 |"), "{text}");
        assert!(text.contains("gone list no tests"), "{text}");
    }

    #[test]
    fn a_boot_gets_time_for_every_test() {
        let files = vec![
            ("bin/x".to_string(), Vec::new()),
            (
                "ltp/runtest/syscalls".to_string(),
                b"# a\nabort01 abort01\naccess01 access01\n".to_vec(),
            ),
        ];
        assert_eq!(timeout(600, &files), 606);
        assert_eq!(timeout(600, &[]), 600);
    }

    #[test]
    fn the_shipped_pin_reads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ltp.toml");
        let pin = Pin::load(&path).unwrap();
        assert!(
            pin.url
                .ends_with(&format!("ltp-full-{}.tar.xz", pin.version))
        );
    }
}
