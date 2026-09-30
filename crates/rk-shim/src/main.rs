//! `rk-cc`, the compiler shim.
//!
//! Runs the real compiler with the arguments it was given, passes standard output through
//! untouched, copies standard error through while keeping its first KiB, and appends one record
//! to `compile.jsonl`. For a probe it also keeps what went in on standard input and what came out
//! on standard output, so that the question can be asked again and its answer compared. It prints nothing of its own unless it cannot run the compiler at all or
//! `RK_TWICE` finds a difference, because kbuild probes judge a compiler by what it writes on
//! standard error, and a shim that chatters there changes the answers it is meant to record.

use rk_shim::args::{self, Mode};
use rk_shim::bringup;
use rk_shim::config::{RECORDED_ENV, ShimConfig};
use rk_shim::digest::sha256_file;
use rk_shim::record::{CompileRecord, FileDigest, Twice};
use rk_shim::usage::{self, Usage};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// How much of standard error goes on the record.
const STDERR_KEEP: usize = 1024;

/// How much of a probe's standard input and output goes on the record.
const PROBE_KEEP: usize = 64 * 1024;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let shim_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let Some(config) = ShimConfig::load(shim_dir.as_deref(), &|key| std::env::var(key).ok()) else {
        eprintln!("rk-cc: no compiler to run: set RK_REAL_CC or put rk-cc.toml next to the shim");
        return ExitCode::from(127);
    };
    if let Err(e) = bringup::check(&config.bringup) {
        eprintln!("rk-cc: RK_BRINGUP: {e}");
        return ExitCode::from(127);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let rest = &argv[1..];
    let invocation = args::read(rest, &cwd, &|p| p.is_file());

    let probe = args::is_probe(rest, &invocation);
    let delegated = bringup::class_of(rest, &invocation, &config.bringup);
    let compiler = match &delegated {
        Some(class) if config.bringup_cc.as_os_str().is_empty() => {
            eprintln!("rk-cc: RK_BRINGUP={class} needs RK_BRINGUP_CC to name a compiler");
            return ExitCode::from(127);
        }
        Some(_) => config.bringup_cc.clone(),
        None => config.real.clone(),
    };
    let passed = bringup::passed_on(rest, delegated.is_some());

    let inputs = digests(&cwd, &invocation.inputs);
    let trace_file =
        (config.rucc_trace && delegated.is_none() && invocation.compiles_c()).then(trace_path);
    let added: Vec<String> = trace_file
        .iter()
        .map(|path| format!("-frucc-trace={}", path.display()))
        .collect();

    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let before = usage::children();
    let clock = Instant::now();
    let stdin = probe_input(probe, rest);
    let run = match run(&compiler, &passed, &added, true, stdin.as_deref(), probe) {
        Ok(run) => run,
        Err(e) => {
            eprintln!("rk-cc: could not run {}: {e}", compiler.display());
            return ExitCode::from(127);
        }
    };
    let wall_seconds = clock.elapsed().as_secs_f64();
    let spent = match (usage::children(), before) {
        (Some(after), Some(before)) => Some(after.since(before)),
        (after, _) => after,
    };

    let outputs = digests(&cwd, &invocation.outputs);
    let rucc = trace_file.as_deref().map(take_trace).unwrap_or_default();

    let mut twice = None;
    if config.twice
        && run.exit == Some(0)
        && !probe
        && matches!(invocation.mode, Mode::Compile | Mode::Assemble)
        && !outputs.is_empty()
    {
        twice = Some(compile_again(&compiler, &passed, &cwd, &outputs));
    }

    let record = CompileRecord {
        started,
        argv,
        compiler: compiler.display().to_string(),
        added,
        cwd: cwd.display().to_string(),
        env: recorded_env(),
        inputs,
        outputs,
        wall_seconds,
        user_seconds: spent.map(|u: Usage| u.user_seconds),
        system_seconds: spent.map(|u| u.system_seconds),
        peak_rss_kb: spent.map(|u| u.peak_rss_kb),
        exit: run.exit,
        signal: run.signal,
        stderr: String::from_utf8_lossy(&run.stderr_head).into_owned(),
        stdin: text_head(stdin.as_deref().unwrap_or_default()),
        stdout: text_head(&run.stdout_head),
        rucc,
        twice: twice.clone(),
        probe,
        delegated,
    };
    if !config.log.as_os_str().is_empty() {
        // A lost record is a hole in the trace, but failing the compile over it would turn a full
        // disk into a compiler bug.
        let _ = record.append_to(&config.log);
    }

    if let Some(Twice {
        identical: false,
        differing,
    }) = twice
    {
        eprintln!(
            "rk-cc: RK_TWICE: a second compile wrote different bytes to {}",
            differing.join(", ")
        );
        return ExitCode::from(1);
    }
    exit_code(&run)
}

/// The shim exits the way the compiler did, with 128 plus the signal when it was killed.
fn exit_code(run: &Run) -> ExitCode {
    match (run.exit, run.signal) {
        (Some(code), _) => ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1)),
        (None, Some(signal)) => ExitCode::from(u8::try_from(128 + signal).unwrap_or(1)),
        (None, None) => ExitCode::from(1),
    }
}

/// The first [`PROBE_KEEP`] bytes as text.
fn text_head(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(PROBE_KEEP)]).into_owned()
}

/// A probe's standard input, read whole to be handed on, when it reads one. It is a line or two
/// from echo or printf. A terminal is left alone, since reading it would wait for someone to type.
fn probe_input(probe: bool, args: &[String]) -> Option<Vec<u8>> {
    (probe && args::reads_stdin(args) && !std::io::stdin().is_terminal()).then(|| {
        let mut buffer = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut buffer);
        buffer
    })
}

/// How a compiler run ended.
struct Run {
    exit: Option<i32>,
    signal: Option<i32>,
    stderr_head: Vec<u8>,
    stdout_head: Vec<u8>,
}

/// Run the compiler, copying its standard error to ours when `echo` is set. `stdin` is fed to it
/// when given, and its standard output is kept as well as passed on when `keep_stdout` is set.
fn run(
    real: &Path,
    args: &[String],
    added: &[String],
    echo: bool,
    stdin: Option<&[u8]>,
    keep_stdout: bool,
) -> std::io::Result<Run> {
    let mut child = Command::new(real)
        .args(args)
        .args(added)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stdout(match (echo, keep_stdout) {
            (_, true) => Stdio::piped(),
            (true, false) => Stdio::inherit(),
            (false, false) => Stdio::null(),
        })
        .stderr(Stdio::piped())
        .spawn()?;
    let feeder = child
        .stdin
        .take()
        .zip(stdin.map(<[u8]>::to_vec))
        .map(|(mut pipe, bytes)| {
            // The pipe closes when the thread ends, which is the end of input the compiler waits for.
            std::thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            })
        });
    let stdout = child
        .stdout
        .take()
        .map(|pipe| copy_keeping(pipe, std::io::stdout(), echo, PROBE_KEEP));
    let stderr = copy_keeping(
        child.stderr.take().expect("stderr was piped"),
        std::io::stderr(),
        echo,
        STDERR_KEEP,
    );
    let status = child.wait()?;
    if let Some(feeder) = feeder {
        let _ = feeder.join();
    }
    Ok(Run {
        exit: status.code(),
        signal: signal_of(status),
        stderr_head: stderr.join().unwrap_or_default(),
        stdout_head: stdout
            .map(|t| t.join().unwrap_or_default())
            .unwrap_or_default(),
    })
}

/// Copy a pipe to one of our streams on a thread when `echo` is set, and keep its first `keep`
/// bytes.
fn copy_keeping(
    mut pipe: impl Read + Send + 'static,
    mut ours: impl Write + Send + 'static,
    echo: bool,
    keep: usize,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut head = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if echo {
                        let _ = ours.write_all(&buffer[..n]);
                    }
                    let room = keep.saturating_sub(head.len());
                    head.extend_from_slice(&buffer[..n.min(room)]);
                }
            }
        }
        let _ = ours.flush();
        head
    })
}

#[cfg(unix)]
fn signal_of(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn signal_of(_status: std::process::ExitStatus) -> Option<i32> {
    None
}

/// Compile again, quietly, and compare every output with the first compile's.
fn compile_again(real: &Path, args: &[String], cwd: &Path, first: &[FileDigest]) -> Twice {
    let ran = run(real, args, &[], false, None, false);
    if !matches!(ran, Ok(Run { exit: Some(0), .. })) {
        return Twice {
            identical: false,
            differing: vec!["(the second compile failed)".to_string()],
        };
    }
    let names: Vec<String> = first.iter().map(|d| d.path.clone()).collect();
    let second = digests(cwd, &names);
    let differing: Vec<String> = first
        .iter()
        .zip(&second)
        .filter(|(a, b)| a.sha256 != b.sha256)
        .map(|(a, _)| a.path.clone())
        .collect();
    Twice {
        identical: differing.is_empty(),
        differing,
    }
}

fn digests(cwd: &Path, paths: &[String]) -> Vec<FileDigest> {
    paths
        .iter()
        .map(|path| FileDigest {
            path: path.clone(),
            sha256: sha256_file(&cwd.join(path)).unwrap_or_default(),
        })
        .collect()
}

fn recorded_env() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(key, _)| {
            RECORDED_ENV.contains(&key.as_str())
                || key.starts_with("RUCC_")
                || key.starts_with("KBUILD_")
        })
        .collect()
}

/// A fresh file name for rucc to write its trace line into.
fn trace_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    std::env::temp_dir().join(format!("rk-cc-{}-{nanos}.jsonl", std::process::id()))
}

/// Read and remove the trace file. Lines that are not JSON are kept as strings, so that an
/// older or newer rucc writing something unexpected shows up on the record instead of vanishing.
fn take_trace(path: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let _ = std::fs::remove_file(path);
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|_| serde_json::Value::String(line.into()))
        })
        .collect()
}
