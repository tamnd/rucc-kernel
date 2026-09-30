//! `toolchains.toml` and `rk personas check`.
//!
//! Every era names a reference container, built from `provision/eras/<name>/` and pinned here by
//! digest. The check runs each container and reads back what it holds: the GCC version, the
//! assembler version and whether GCC finds a plugin directory. The versions must be the ones
//! `personas.toml` says the era was built with, and there must be no plugin directory, since
//! that keeps `GCC_PLUGINS` off for the reference as it is for rucc (plan 5.4). A Debian archive
//! rebuild that moves a patch level shows up here and not as a strange diff three steps later.

use crate::personas::Era;
use serde::Deserialize;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

/// The file.
#[derive(Debug, Clone, Deserialize)]
pub struct Toolchains {
    /// Every era container.
    #[serde(rename = "container")]
    pub containers: Vec<Container>,
}

/// One container.
#[derive(Debug, Clone, Deserialize)]
pub struct Container {
    /// The name `personas.toml` uses, `rk-era-<release>`.
    pub name: String,
    /// The image without a tag.
    pub image: String,
    /// The pinned digest, empty until the first push.
    #[serde(default)]
    pub digest: String,
}

impl Container {
    /// The image by digest, or by the `latest` tag while no digest is pinned.
    #[must_use]
    pub fn reference(&self) -> String {
        if self.digest.is_empty() {
            format!("{}:latest", self.image)
        } else {
            format!("{}@{}", self.image, self.digest)
        }
    }
}

impl Toolchains {
    /// Read `toolchains.toml`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parse the text of `toolchains.toml`.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// A container by name.
    pub fn get(&self, name: &str) -> Result<&Container, String> {
        self.containers
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| format!("{name} is not in toolchains.toml"))
    }
}

/// The shell script run in the container, one keyed line per fact. The GCC version comes from
/// the `__GNUC__` macros, which is what the kernel reads, because `-dumpversion` prints only part
/// of it on Debian's 4.7 to 4.9 and on everything from 7, and `-dumpfullversion` is new in 7.
/// The plugin line names the plugin directory only when it has `include/plugin-version.h`, which
/// is the file kbuild tests for. The directory itself ships with every GCC package, and the
/// header only with `gcc-<v>-plugin-dev`.
pub const PROBE: &str = "echo \"gcc $(gcc -E -dM - </dev/null | awk '\
$2 == \"__GNUC__\" { a = $3 } $2 == \"__GNUC_MINOR__\" { b = $3 } \
$2 == \"__GNUC_PATCHLEVEL__\" { c = $3 } END { print a \".\" b \".\" c }')\"; \
echo \"as $(as --version | head -n 1)\"; \
p=$(gcc -print-file-name=plugin); \
if [ -e \"$p/include/plugin-version.h\" ]; then echo \"plugin $p\"; else echo \"plugin none\"; fi";

/// What a container holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Found {
    /// The GCC version from its macros.
    pub gcc: String,
    /// The binutils version from the first line of `as --version`.
    pub binutils: String,
    /// The plugin directory when it has the headers kbuild looks for, or `none`.
    pub plugin: String,
}

/// The first word of a line that looks like a version: a digit first and at least one dot.
fn version_in(line: &str) -> Option<&str> {
    line.split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'))
}

impl Found {
    /// Read the output of [`PROBE`].
    #[must_use]
    pub fn parse(output: &str) -> Self {
        let mut found = Self::default();
        for line in output.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            let value = value.trim();
            match key {
                "gcc" => found.gcc = value.to_string(),
                "as" => found.binutils = version_in(value).unwrap_or("").to_string(),
                "plugin" => found.plugin = value.to_string(),
                _ => {}
            }
        }
        found
    }
}

/// Whether a found version is the wanted one: the same, or the wanted one followed by more
/// parts, since Debian's binutils print a snapshot date after the release.
#[must_use]
pub fn matches(found: &str, wanted: &str) -> bool {
    found == wanted
        || found
            .strip_prefix(wanted)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// What is wrong with a container for an era, empty when nothing is.
#[must_use]
pub fn problems(era: &Era, found: &Found) -> Vec<String> {
    let mut out = Vec::new();
    if !matches(&found.gcc, &era.reference.gcc) {
        out.push(format!(
            "gcc is {}, not {}",
            or_nothing(&found.gcc),
            era.reference.gcc
        ));
    }
    if !matches(&found.binutils, &era.reference.binutils) {
        out.push(format!(
            "binutils is {}, not {}",
            or_nothing(&found.binutils),
            era.reference.binutils
        ));
    }
    if found.plugin != "none" {
        out.push(format!("gcc finds plugin headers in {}", found.plugin));
    }
    out
}

fn or_nothing(s: &str) -> &str {
    if s.is_empty() { "missing" } else { s }
}

/// Run [`PROBE`] in an image with `engine`, which is `docker` or `podman`.
pub fn probe(engine: &str, image: &str) -> Result<Found, String> {
    let out = Command::new(engine)
        .args(["run", "--rm", "--pull=missing", image, "sh", "-c", PROBE])
        .output()
        .map_err(|e| format!("running {engine}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{engine} run {image} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(Found::parse(&String::from_utf8_lossy(&out.stdout)))
}

/// One row of the check.
#[derive(Debug, Clone)]
pub struct Checked {
    /// The era.
    pub era: String,
    /// The container name.
    pub container: String,
    /// What was found, or why nothing was.
    pub found: Result<Found, String>,
    /// What is wrong.
    pub problems: Vec<String>,
}

/// The check as markdown.
#[must_use]
pub fn report(rows: &[Checked]) -> String {
    let mut s = String::from(
        "| era | container | gcc | binutils | plugins | result |\n|---|---|---|---|---|---|\n",
    );
    for r in rows {
        let (gcc, binutils, plugins) = match &r.found {
            Ok(f) => (
                f.gcc.as_str(),
                f.binutils.as_str(),
                if f.plugin == "none" { "none" } else { "found" },
            ),
            Err(_) => ("", "", ""),
        };
        let result = match &r.found {
            Err(e) => e.clone(),
            Ok(_) if r.problems.is_empty() => "ok".to_string(),
            Ok(_) => r.problems.join("; "),
        };
        let _ = writeln!(
            s,
            "| {} | {} | {gcc} | {binutils} | {plugins} | {result} |",
            r.era, r.container
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::personas::Personas;

    fn era(id: &str) -> Era {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../personas.toml"))
                .unwrap();
        Personas::parse(&text)
            .unwrap()
            .eras
            .into_iter()
            .find(|e| e.id == id)
            .unwrap()
    }

    #[test]
    fn the_committed_file_names_every_container_in_personas() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../toolchains.toml"
        ))
        .unwrap();
        let toolchains = Toolchains::parse(&text).unwrap();
        for id in ["E0", "E3", "E4", "E5", "E6", "E7", "E8", "E9", "E10", "E11"] {
            let container = toolchains.get(&era(id).reference.container).unwrap();
            let release = container.name.trim_start_matches("rk-era-");
            let dockerfile = format!(
                "{}/../../provision/eras/{release}/Dockerfile",
                env!("CARGO_MANIFEST_DIR")
            );
            assert!(Path::new(&dockerfile).is_file(), "{dockerfile}");
        }
    }

    #[test]
    fn trixie_output_reads_and_passes() {
        let found = Found::parse(
            "gcc 14.2.0\nas GNU assembler (GNU Binutils for Debian) 2.44\nplugin none\n",
        );
        assert_eq!(found.gcc, "14.2.0");
        assert_eq!(found.binutils, "2.44");
        assert!(problems(&era("E11"), &found).is_empty());
    }

    #[test]
    fn an_old_assembler_banner_still_reads() {
        let found =
            Found::parse("gcc 3.4.6\nas GNU assembler 2.17 Debian GNU/Linux\nplugin none\n");
        assert_eq!(found.gcc, "3.4.6");
        assert_eq!(found.binutils, "2.17");
        assert_eq!(
            problems(&era("E3"), &found),
            vec!["binutils is 2.17, not 2.16".to_string()]
        );
    }

    #[test]
    fn a_snapshot_binutils_matches_its_release_but_a_plugin_directory_fails() {
        assert!(matches("2.18.0.20080103", "2.18"));
        assert!(!matches("2.180", "2.18"));
        let found = Found {
            gcc: "12.2.0".to_string(),
            binutils: "2.40".to_string(),
            plugin: "/usr/lib/gcc/x86_64-linux-gnu/12/plugin".to_string(),
        };
        let p = problems(&era("E10"), &found);
        assert_eq!(p.len(), 1);
        assert!(p[0].starts_with("gcc finds plugin headers"));
        assert!(
            report(&[Checked {
                era: "E10".to_string(),
                container: "rk-era-bookworm".to_string(),
                found: Ok(found),
                problems: p,
            }])
            .contains("| found |")
        );
    }
}
