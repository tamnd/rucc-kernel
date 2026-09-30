//! The `rk` command line: fetch pinned Linux trees, build them with rucc and with a reference
//! compiler, boot and test what comes out, and record what happened.

mod cli;
mod kernelorg;
mod personas;
mod pins;
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

/// List every pin with its era and persona, which fails when a pin falls in no era or in two.
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
    Ok(ExitCode::SUCCESS)
}
