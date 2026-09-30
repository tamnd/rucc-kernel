//! The `rk` command line: fetch pinned Linux trees, build them with rucc and with a reference
//! compiler, boot and test what comes out, and record what happened.

mod asm;
mod baseline;
mod boot;
mod build;
mod cli;
mod demands;
mod flags;
mod kconfig;
mod kernelorg;
mod modules;
mod objects;
mod objtool;
mod personas;
mod pins;
mod probes;
mod repo;
mod sections;
mod sets;
mod symvers;
mod syntax;
mod toolchains;
mod vec;
mod why;

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
            "personas" if args.target.as_deref() == Some("check") => personas_check(&repo, &args),
            "personas" if args.target.is_none() => personas_command(&repo),
            "personas" => Err("rk personas takes only the word check".to_string()),
            "build" => build_command(&repo, &args),
            "config-diff" => config_diff(&repo, &args),
            "probes" => probes_command(&args),
            "flags-diff" => flags_diff(&repo, &args),
            "syntax" => syntax_command(&repo, &args),
            "sections-diff" => sections_diff(&args),
            "symvers-diff" => symvers_diff(&args),
            "vec-audit" => vec_audit(&args),
            "modules-audit" => modules_audit(&args),
            "objtool-report" => objtool_report(&args),
            "demands" => demands_command(&args),
            "boot" => boot_command(&repo, &args),
            "asm-inventory" => asm_inventory(&repo, &args),
            "baseline" => baseline_command(&repo, &args),
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

/// `rk personas check`: run every era's reference container and compare what it holds with
/// `personas.toml`. Eras sharing a container are checked with one run.
fn personas_check(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let personas = personas::Personas::load(&repo.file("personas.toml"))?;
    let toolchains = toolchains::Toolchains::load(&repo.file("toolchains.toml"))?;
    let engine = args.get("engine").unwrap_or("docker");
    let wanted: Option<Vec<&str>> = args
        .get("era")
        .map(|e| e.split(',').map(str::trim).collect());
    let mut runs: std::collections::BTreeMap<String, Result<toolchains::Found, String>> =
        std::collections::BTreeMap::new();
    let mut rows = Vec::new();
    for era in &personas.eras {
        if wanted
            .as_ref()
            .is_some_and(|w| !w.contains(&era.id.as_str()))
        {
            continue;
        }
        let container = toolchains.get(&era.reference.container)?;
        let found = runs
            .entry(container.name.clone())
            .or_insert_with(|| {
                let image = container.reference();
                eprintln!("checking {} in {image}", era.id);
                toolchains::probe(engine, &image)
            })
            .clone();
        let problems = match &found {
            Ok(f) => toolchains::problems(era, f),
            Err(e) => vec![e.clone()],
        };
        rows.push(toolchains::Checked {
            era: era.id.clone(),
            container: container.name.clone(),
            found,
            problems,
        });
    }
    if rows.is_empty() {
        return Err("no era matches --era".to_string());
    }
    let report = toolchains::report(&rows);
    print!("{report}");
    step_summary(&format!("### rk personas check\n\n{report}"));
    let failed = rows.iter().filter(|r| !r.problems.is_empty()).count();
    if failed > 0 {
        eprintln!("{failed} of {} eras do not match personas.toml", rows.len());
        return Ok(ExitCode::FAILURE);
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

/// The build that the options of `rk build` or `rk baseline` describe.
fn build_plan(repo: &Repo, args: &Args) -> Result<build::Plan, String> {
    let pins = pins::Pins::load(&repo.file("pins.toml"))?;
    let pin = pins.get(args.target.as_deref())?.clone();
    let personas = personas::Personas::load(&repo.file("personas.toml"))?;
    let era = personas.era_for(&pin.version)?.clone();
    let rows = personas::Rows::load(&repo.file("rows.toml"))?;
    let row = rows.get(args.get("row").unwrap_or("X64"))?.clone();
    let config = args.get("config").unwrap_or("defconfig").to_string();
    let compiler = build::Compiler::identify(
        args.get("cc")
            .ok_or("--cc is needed, the compiler under test or the reference")?,
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
    Ok(plan)
}

/// Configure and build one kernel through the shim.
fn build_command(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let plan = build_plan(repo, args)?;
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
    if args.has("why") {
        let unexplained: Vec<kconfig::Difference> = differences
            .iter()
            .filter(|d| d.reason.is_none())
            .cloned()
            .collect();
        print!("{}", config_why(repo, args, &unexplained)?);
    }
    Ok(if differences.iter().all(|d| d.reason.is_some()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// A build directory's `build.json`.
fn outcome_of(dir: &std::path::Path) -> Result<build::Outcome, String> {
    let path = dir.join("build.json");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("reading {}: {e}", path.display()))
}

/// The probes of a build directory by question.
fn probes_of(dir: &std::path::Path) -> Result<BTreeMap<String, probes::Answers>, String> {
    let path = dir.join("compile.jsonl");
    let (records, _) =
        rk_shim::record::read_log(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    Ok(probes::collect(&records))
}

/// The Kconfig expressions and probes behind each difference, for `--why`. Both sides must be
/// build directories, and the tree is `--source` or the one the reference's `build.json` names.
fn config_why(
    repo: &Repo,
    args: &Args,
    differences: &[kconfig::Difference],
) -> Result<String, String> {
    let reference = std::path::Path::new(args.get("reference").unwrap_or_default());
    let other = std::path::Path::new(args.get("other").unwrap_or_default());
    if !reference.is_dir() || !other.is_dir() {
        return Err("--why needs --reference and --other to be build directories".to_string());
    }
    let outcome = outcome_of(reference)?;
    let tree = match args.get("source") {
        Some(dir) => std::path::PathBuf::from(dir),
        None if !outcome.source.as_os_str().is_empty() => outcome.source.clone(),
        None => pins::Pins::load(&repo.file("pins.toml"))?
            .get(Some(&outcome.version))?
            .source_dir(),
    };
    let rows = personas::Rows::load(&repo.file("rows.toml"))?;
    let srcarch = boot::srcarch(&rows.get(&outcome.row)?.arch).to_string();
    let symbols = why::load(&tree, &srcarch)?;
    let disagreements = probes::compare(&probes_of(reference)?, &probes_of(other)?);
    Ok(why::report(
        &why::explain(differences, &symbols, &disagreements),
        &disagreements,
    ))
}

/// Compare the flags every unit was compiled with in two builds.
fn flags_diff(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let side = |name: &str| -> Result<flags::Commands, String> {
        let dir = args
            .get(name)
            .ok_or_else(|| format!("rk flags-diff needs --{name}"))?;
        let dir = std::path::Path::new(dir);
        let raw = flags::load(dir)?;
        // The commands name the output directory the build ran in, which is where the shim was,
        // and the tree build.json names. The directory given may be a copy somewhere else.
        let ran_in = raw
            .values()
            .filter_map(|words| words.first())
            .find_map(|cc| cc.strip_suffix("/rk-bin/rk-cc"))
            .map(str::to_string)
            .unwrap_or_default();
        let source = outcome_of(dir)
            .map(|o| o.source.display().to_string())
            .unwrap_or_default();
        let here = std::fs::canonicalize(dir)
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let dirs = [
            (ran_in.as_str(), "OUT"),
            (here.as_str(), "OUT"),
            (source.as_str(), "SRC"),
        ];
        Ok(raw
            .into_iter()
            .map(|(object, words)| (object, flags::normalize(&words, &dirs)))
            .collect())
    };
    let reference = side("reference")?;
    let other = side("other")?;
    if reference.is_empty() {
        return Err("the reference has no .cmd files; was it built?".to_string());
    }
    let divergences = kconfig::Divergences::load(&repo.file("config-divergences.toml"))?;
    let comparison = flags::compare(&reference, &other, &divergences);
    print!("{}", flags::report(&comparison));
    Ok(if comparison.clean() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// The exit code of a report: success when it is clean.
fn verdict(clean: bool) -> ExitCode {
    if clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Compare the sections and kernel tables of every object in two builds.
fn sections_diff(args: &Args) -> Result<ExitCode, String> {
    let reference = std::path::Path::new(
        args.get("reference")
            .ok_or("rk sections-diff needs --reference")?,
    );
    let reference = objects::load_or_scan(reference, sections::scan)?;
    if let Some(path) = args.get("save") {
        objects::save(std::path::Path::new(path), &reference)?;
        eprintln!("saved {} objects to {path}", reference.len());
    }
    let Some(other) = args.get("other") else {
        return if args.get("save").is_some() {
            Ok(ExitCode::SUCCESS)
        } else {
            Err("rk sections-diff needs --other, or --save to keep the reference".to_string())
        };
    };
    if reference.is_empty() {
        return Err("the reference has no objects; was it built?".to_string());
    }
    let other = objects::load_or_scan(std::path::Path::new(other), sections::scan)?;
    let comparison = sections::compare(&reference, &other);
    print!("{}", sections::report(&comparison));
    Ok(verdict(comparison.clean()))
}

/// Compare `Module.symvers` and the global symbols of `System.map` in two builds.
fn symvers_diff(args: &Args) -> Result<ExitCode, String> {
    let side = |name: &str| {
        args.get(name)
            .map(|d| symvers::load(std::path::Path::new(d)))
            .ok_or_else(|| format!("rk symvers-diff needs --{name}"))
    };
    let (rs, rm) = side("reference")?;
    let (os, om) = side("other")?;
    if rm.is_empty() {
        return Err("the reference has no System.map; was it linked?".to_string());
    }
    let comparison = symvers::compare((&rs, &rm), (&os, &om));
    print!("{}", symvers::report(&comparison));
    Ok(verdict(comparison.clean()))
}

/// Look for vector and x87 instructions in the no-FPU units of a build.
fn vec_audit(args: &Args) -> Result<ExitCode, String> {
    let build = args.get("build").ok_or("rk vec-audit needs --build")?;
    let build = objects::load_or_scan(std::path::Path::new(build), vec::scan)?;
    if let Some(path) = args.get("save") {
        objects::save(std::path::Path::new(path), &build)?;
    }
    let reference = args
        .get("reference")
        .map(|r| objects::load_or_scan(std::path::Path::new(r), vec::scan))
        .transpose()?;
    let findings = vec::findings(&build, reference.as_ref());
    print!(
        "{}",
        vec::report(build.len(), &findings, reference.is_some())
    );
    Ok(verdict(findings.is_empty()))
}

/// Check the relocations, vermagic and CRCs of every module of a build.
fn modules_audit(args: &Args) -> Result<ExitCode, String> {
    let build = args.get("build").ok_or("rk modules-audit needs --build")?;
    let audited = modules::audit(std::path::Path::new(build))?;
    print!("{}", modules::report(&audited));
    Ok(verdict(audited.values().all(|(_, p)| p.is_empty())))
}

/// Bucket and compare the objtool warnings of two builds.
fn objtool_report(args: &Args) -> Result<ExitCode, String> {
    let side = |name: &str| {
        let path = args
            .get(name)
            .ok_or_else(|| format!("rk objtool-report needs --{name}"))?;
        objtool::load(std::path::Path::new(path))
    };
    let buckets = objtool::bucket(&side("reference")?, &side("other")?);
    print!("{}", objtool::report(&buckets));
    Ok(verdict(objtool::clean(&buckets)))
}

/// Replay every unit of a reference build through another compiler's front end.
fn syntax_command(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let build = std::path::PathBuf::from(args.get("build").ok_or("rk syntax needs --build")?);
    let outcome = outcome_of(&build)?;
    let compiler = build::Compiler::identify(args.get("cc").unwrap_or("rucc"))?;
    let persona = if compiler.rucc {
        let gnuc = if outcome.gnuc.is_empty() {
            personas::Personas::load(&repo.file("personas.toml"))?
                .eras
                .into_iter()
                .find(|e| e.id == outcome.era)
                .map(|e| e.gnuc)
                .ok_or_else(|| format!("{} is not in personas.toml", outcome.era))?
        } else {
            outcome.gnuc.clone()
        };
        vec![format!("-fgnuc-version={gnuc}")]
    } else {
        Vec::new()
    };
    let known = match args.get("allow") {
        Some(path) => syntax::Known::load(std::path::Path::new(path))?,
        None if repo.file("syntax-known.toml").is_file() => {
            syntax::Known::load(&repo.file("syntax-known.toml"))?
        }
        None => syntax::Known::default(),
    };
    let jobs = match args.get("jobs") {
        Some(n) => n
            .parse()
            .map_err(|_| format!("--jobs {n} is not a number"))?,
        None => std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
    };
    let fallback = pins::Pins::load(&repo.file("pins.toml"))?
        .get(Some(&outcome.version))?
        .source_dir();
    let tree = syntax::tree(&outcome.source, fallback);
    let log = build.join("compile.jsonl");
    let (records, _) =
        rk_shim::record::read_log(&log).map_err(|e| format!("reading {}: {e}", log.display()))?;
    let results = syntax::run(&records, &tree, &compiler.path, &persona, jobs);
    let summary = syntax::Summary::new(&results, &known);
    print!("{}", summary.report());
    Ok(if summary.passed() {
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
        probes_of(std::path::Path::new(dir))
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

/// Replay a reference build's units to text and count the instructions the kernel writes itself.
fn asm_inventory(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let build =
        std::path::PathBuf::from(args.get("build").ok_or("rk asm-inventory needs --build")?);
    let text = std::fs::read_to_string(build.join("build.json"))
        .map_err(|e| format!("reading {}: {e}", build.join("build.json").display()))?;
    let outcome: build::Outcome =
        serde_json::from_str(&text).map_err(|e| format!("reading build.json: {e}"))?;
    let pins = pins::Pins::load(&repo.file("pins.toml"))?;
    let tree = pins.get(Some(&outcome.version))?.source_dir();
    let rows = personas::Rows::load(&repo.file("rows.toml"))?;
    let x86 = boot::srcarch(&rows.get(&outcome.row)?.arch) == "x86";
    let jobs = match args.get("jobs") {
        Some(n) => n
            .parse()
            .map_err(|_| format!("--jobs {n} is not a number"))?,
        None => std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
    };
    let log = build.join("compile.jsonl");
    let (records, _) =
        rk_shim::record::read_log(&log).map_err(|e| format!("reading {}: {e}", log.display()))?;
    let inventory = asm::collect(&records, &tree, x86, jobs);
    let report = inventory.report();
    let path = build.join("asm-inventory.md");
    std::fs::write(&path, &report).map_err(|e| format!("writing {}: {e}", path.display()))?;
    println!("{}", report.lines().next().unwrap_or_default());
    println!("wrote {}", path.display());
    Ok(ExitCode::SUCCESS)
}

/// Build and boot with the reference several times, and write the result under
/// `results/baseline`.
fn baseline_command(repo: &Repo, args: &Args) -> Result<ExitCode, String> {
    let plan = build_plan(repo, args)?;
    let runs: usize = match args.get("runs") {
        Some(n) => n
            .parse()
            .map_err(|_| format!("--runs {n} is not a number"))?,
        None => 3,
    };
    let timeout = match args.get("timeout") {
        Some(n) => n
            .parse()
            .map_err(|_| format!("--timeout {n} is not a number"))?,
        None => 600,
    };
    let mut results = Vec::new();
    for run in 1..=runs {
        let out = plan.out.join(format!("run{run}"));
        eprintln!("rk: baseline run {run} of {runs} in {}", out.display());
        let built = build::run(&build::Plan {
            out: out.clone(),
            ..plan.clone()
        })?;
        let booted = if built.built {
            let initramfs = out.join("initramfs.cpio");
            write_initramfs(&initramfs, args.get("busybox"))?;
            Some(boot::run(&boot::Plan {
                build: std::fs::canonicalize(&out).map_err(|e| e.to_string())?,
                row: plan.row.clone(),
                initramfs,
                timeout,
                append: String::new(),
            })?)
        } else {
            None
        };
        results.push(baseline::Run::from(&built, booted.as_ref()));
    }
    let fragment = plan.fragment.as_ref().map(|(name, _)| name.clone());
    let baseline = baseline::Baseline::new(&plan, fragment, results);
    let path = repo
        .file("results")
        .join("baseline")
        .join(baseline.file_name());
    baseline.write(&path)?;
    let summary = baseline.summary();
    print!("{summary}");
    step_summary(&summary);
    println!("wrote {}", path.display());
    Ok(if baseline.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}
