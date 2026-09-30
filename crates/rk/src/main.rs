//! The `rk` command line: fetch pinned Linux trees, build them with rucc and with a reference
//! compiler, boot and test what comes out, and record what happened.

mod boot;
mod build;
mod cli;
mod demands;
mod kconfig;
mod kernelorg;
mod personas;
mod pins;
mod probes;
mod repo;
mod sets;

use cli::Args;
use repo::Repo;
use std::collections::BTreeMap;
use std::process::ExitCode;

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let result = Args::parse(&words).and_then(|args| {
        if args.command == "help" || args.has("help") {
            print!("{}", cli::USAGE);
            return Ok(ExitCode::SUCCESS);
        }
        if args.command == "version" {
            println!("rk {}", env!("CARGO_PKG_VERSION"));
            return Ok(ExitCode::SUCCESS);
        }
        let repo = Repo::find()?;
        match args.command.as_str() {
            "fetch" => fetch(&repo, &args),
            "sets" => sets_command(&repo, &args),
            "personas" => personas_command(&repo),
            "build" => build_command(&repo, &args),
            "config-diff" => config_diff(&repo, &args),
            "probes" => probes_command(&args),
            "demands" => demands_command(&args),
            "boot" => boot_command(&repo, &args),
            "initramfs" => initramfs_command(&args).map(|_| ExitCode::SUCCESS),
            _ => unreachable!("the parser only accepts known commands"),
        }
    });
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("rk: {message}");
            ExitCode::from(2)
        }
    }
}

fn fetch(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let pins = pins::Pins::load(&repo.file("pins.toml"))?;
    let chosen: Vec<&pins::Pin> = if args.has("all") {
        pins.pins.iter().collect()
    } else if let Some(set) = args.get("set") {
        let chosen = pins.in_set(set);
        if chosen.is_empty() {
            return Err(format!("no pin is in the set {set}"));
        }
        chosen
    } else {
        vec![pins.get(args.target.as_deref())?]
    };
    for pin in chosen {
        let tree = pins::fetch(pin, !args.has("no-upstream-check"))?;
        println!("{}", tree.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn sets_command(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let sets = sets::Sets::load(&repo.file("sets.toml"))?;
    let text = match args.get("releases") {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?,
        None => kernelorg::fetch_text(kernelorg::RELEASES_URL)?,
    };
    let releases = kernelorg::parse_releases(&text)?;
    let mut cache: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let new = sets::pins_for(&sets, &releases, &mut |version| {
        let dir = kernelorg::major_dir(version);
        if let Some(sums) = cache.get(&dir) {
            return Ok(sums.clone());
        }
        let sums = kernelorg::parse_sums(&kernelorg::fetch_text(&kernelorg::sums_url(version))?);
        cache.insert(dir, sums.clone());
        Ok(sums)
    })?;
    let path = repo.file("pins.toml");
    let old = pins::Pins::load(&path).ok();
    let lines = sets::describe(old.as_ref(), &new);
    if lines.is_empty() {
        println!("pins.toml matches the sets");
    }
    for line in &lines {
        println!("{line}");
    }
    if args.has("write") && !lines.is_empty() {
        std::fs::write(&path, new.to_file())
            .map_err(|e| format!("writing {}: {e}", path.display()))?;
        println!("wrote {}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

/// List every pin with its era and persona, which fails when a pin falls in no era or in two,
/// and then the rows.
fn personas_command(repo: &Repo) -> Result<ExitCode, String> {
    let personas = personas::Personas::load(&repo.file("personas.toml"))?;
    let pins = pins::Pins::load(&repo.file("pins.toml"))?;
    for pin in &pins.pins {
        let era = personas.era_for(&pin.version)?;
        println!(
            "{:<10} {:<4} -fgnuc-version={:<8} as {:<7} -std={:<6} reference gcc {} binutils {} in {}",
            pin.version,
            era.id,
            era.gnuc,
            era.gnu_as,
            era.std,
            era.reference.gcc,
            era.reference.binutils,
            era.reference.container
        );
        if !era.notes.is_empty() {
            println!("{:<15} {}", "", era.notes);
        }
    }
    let rows = personas::Rows::load(&repo.file("rows.toml"))?;
    println!();
    for row in &rows.rows {
        println!(
            "{:<4} ARCH={:<8} {:<8} {} -M {} -cpu {} console={}",
            row.name, row.arch, row.image, row.qemu, row.machine, row.cpu, row.console
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// A fragment by name, for an era: `configs/<name>.fragment.<era>`.
fn fragment_for(
    repo: &Repo,
    name: Option<&str>,
    era: &str,
) -> Result<Option<(String, std::path::PathBuf)>, String> {
    let Some(name) = name else {
        return Ok(None);
    };
    let path = repo.file("configs").join(format!("{name}.fragment.{era}"));
    if !path.is_file() {
        return Err(format!(
            "no fragment {name} for era {era}: {}",
            path.display()
        ));
    }
    Ok(Some((name.to_string(), path)))
}

/// Configure and build one kernel through the shim.
fn build_command(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let pins = pins::Pins::load(&repo.file("pins.toml"))?;
    let pin = pins.get(args.target.as_deref())?.clone();
    let personas = personas::Personas::load(&repo.file("personas.toml"))?;
    let era = personas.era_for(&pin.version)?.clone();
    let rows = personas::Rows::load(&repo.file("rows.toml"))?;
    let row = rows.get(args.get("row").unwrap_or("X64"))?.clone();
    let config = args.get("config").unwrap_or("defconfig").to_string();
    let compiler = build::Compiler::identify(
        args.get("cc")
            .ok_or("rk build needs --cc, the compiler under test or the reference")?,
    )?;
    let jobs = match args.get("jobs") {
        Some(n) => n
            .parse()
            .map_err(|_| format!("--jobs {n} is not a number"))?,
        None => std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
    };
    let bringup: Vec<String> = args
        .get("bringup")
        .map(|b| b.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    let bringup_cc = match args.get("bringup-cc") {
        Some(cc) => Some(build::Compiler::identify(cc)?.path),
        None if !bringup.is_empty() => return Err("--bringup needs --bringup-cc".to_string()),
        None => None,
    };
    let targets = args.get("targets").map_or_else(
        || vec![row.image.clone()],
        |t| t.split_whitespace().map(str::to_string).collect(),
    );
    let fragment = fragment_for(repo, args.get("fragment"), &era.id)?;
    let config_name = match &fragment {
        Some((name, _)) => format!("{config}+{name}"),
        None => config.clone(),
    };
    let out = args.get("out").map_or_else(
        || {
            repo.file("work").join(format!(
                "{}-{}-{}-{}",
                pin.version,
                row.name,
                config_name,
                compiler.label()
            ))
        },
        std::path::PathBuf::from,
    );
    let source = pins::fetch(&pin, !args.has("no-upstream-check"))?;
    let plan = build::Plan {
        pin,
        source,
        row,
        era,
        config,
        compiler,
        out,
        jobs,
        keep_going: args.has("keep-going"),
        config_only: args.has("config-only"),
        twice: args.has("twice"),
        bringup,
        bringup_cc,
        targets,
        fragment,
    };
    eprintln!(
        "rk: building {} {} {} with {} in {}",
        plan.pin.version,
        plan.row.name,
        plan.config,
        plan.compiler.version,
        plan.out.display()
    );
    let outcome = build::run(&plan)?;
    let summary = build::summary(&outcome);
    print!("{summary}");
    step_summary(&summary);
    let done = outcome.configured && (plan.config_only || outcome.built);
    Ok(if done {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Compare two builds' `.config`, and fail when a difference has no reason on file.
fn config_diff(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let side = |name: &str| -> Result<kconfig::Config, String> {
        let path = args
            .get(name)
            .ok_or_else(|| format!("rk config-diff needs --{name}"))?;
        kconfig::load(std::path::Path::new(path))
    };
    let reference = side("reference")?;
    let other = side("other")?;
    let divergences = kconfig::Divergences::load(&repo.file("config-divergences.toml"))?;
    let differences = kconfig::diff(&reference, &other, &divergences);
    print!("{}", kconfig::report(&differences));
    Ok(if differences.iter().all(|d| d.reason.is_some()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Compare the compiler probes of two builds, and fail when they answer any differently.
fn probes_command(args: &Args) -> Result<ExitCode, String> {
    let side = |name: &str| -> Result<_, String> {
        let dir = args
            .get(name)
            .ok_or_else(|| format!("rk probes needs --{name}"))?;
        let path = std::path::Path::new(dir).join("compile.jsonl");
        let (records, _) = rk_shim::record::read_log(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Ok(probes::collect(&records))
    };
    let reference = side("reference")?;
    let other = side("other")?;
    let disagreements = probes::compare(&reference, &other);
    print!("{}", probes::report(&reference, &other, &disagreements));
    Ok(if disagreements.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Count the failed units of several builds by error, most units first.
fn demands_command(args: &Args) -> Result<ExitCode, String> {
    let builds = args.get("builds").ok_or("rk demands needs --builds")?;
    let limit = match args.get("limit") {
        Some(n) => n
            .parse()
            .map_err(|_| format!("--limit {n} is not a number"))?,
        None => 40,
    };
    let mut census = std::collections::BTreeMap::new();
    for dir in builds.split_whitespace() {
        let path = std::path::Path::new(dir).join("compile.jsonl");
        let (records, _) = rk_shim::record::read_log(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let name = std::path::Path::new(dir)
            .file_name()
            .map_or_else(|| dir.to_string(), |n| n.to_string_lossy().into_owned());
        demands::add(&mut census, &name, &records);
    }
    print!("{}", demands::report(&demands::ranked(census), limit));
    Ok(ExitCode::SUCCESS)
}

/// Write the initramfs for a static busybox, and say where.
fn initramfs_command(args: &Args) -> Result<std::path::PathBuf, String> {
    let out = std::path::PathBuf::from(args.get("out").ok_or("rk initramfs needs --out")?);
    write_initramfs(&out, args.get("busybox"))?;
    println!("{}", out.display());
    Ok(out)
}

/// Write the initramfs for a busybox, `/bin/busybox` by default, which must be static.
fn write_initramfs(out: &std::path::Path, busybox: Option<&str>) -> Result<(), String> {
    let busybox = busybox.unwrap_or("/bin/busybox");
    let bytes = std::fs::read(busybox).map_err(|e| format!("reading {busybox}: {e}"))?;
    if !boot::is_static_elf(&bytes) {
        return Err(format!(
            "{busybox} is not a static ELF program; install busybox-static or pass --busybox"
        ));
    }
    std::fs::write(out, boot::initramfs(&bytes))
        .map_err(|e| format!("writing {}: {e}", out.display()))
}

/// Boot a build under QEMU and run the smoke checks.
fn boot_command(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let build = std::path::PathBuf::from(args.get("build").ok_or("rk boot needs --build")?);
    let build =
        std::fs::canonicalize(&build).map_err(|e| format!("resolving {}: {e}", build.display()))?;
    let row_name = match args.get("row") {
        Some(row) => row.to_string(),
        None => std::fs::read_to_string(build.join("build.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<build::Outcome>(&text).ok())
            .map_or_else(|| "X64".to_string(), |o| o.row),
    };
    let rows = personas::Rows::load(&repo.file("rows.toml"))?;
    let row = rows.get(&row_name)?.clone();
    let initramfs = if let Some(path) = args.get("initramfs") {
        std::path::PathBuf::from(path)
    } else {
        let path = build.join("initramfs.cpio");
        write_initramfs(&path, args.get("busybox"))?;
        path
    };
    let timeout = match args.get("timeout") {
        Some(n) => n
            .parse()
            .map_err(|_| format!("--timeout {n} is not a number"))?,
        None => 300,
    };
    let plan = boot::Plan {
        build,
        row,
        initramfs,
        timeout,
        append: args.get("append").unwrap_or_default().to_string(),
    };
    let outcome = boot::run(&plan)?;
    let summary = boot::summary(&outcome);
    print!("{summary}");
    step_summary(&summary);
    Ok(if outcome.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Add to the GitHub job summary, when there is one.
fn step_summary(text: &str) {
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
        {
            let _ = writeln!(file, "{text}");
        }
    }
}
