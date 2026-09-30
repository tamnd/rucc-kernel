//! What the shim needs to know: which compiler to run and where to write.
//!
//! `rk build` copies `rk-cc` into the build's `bin` directory and writes `rk-cc.toml` next to the
//! copy. The shim reads that file from the directory it was run from, and environment variables
//! of the same meaning override it. The file is what makes the shim work when kbuild calls it from
//! a sub make whose environment has been cleaned, which happens in the boot and vDSO Makefiles.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The name of the file next to the shim.
pub const FILE_NAME: &str = "rk-cc.toml";

/// The shim's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ShimConfig {
    /// The real compiler, as an absolute path. Overridden by `RK_REAL_CC`.
    pub real: PathBuf,
    /// The `compile.jsonl` to append to. Overridden by `RK_COMPILE_LOG`.
    pub log: PathBuf,
    /// Whether to add `-frucc-trace`, decided once by `rk build` after asking the compiler.
    /// Overridden by `RK_RUCC_TRACE`.
    #[serde(default)]
    pub rucc_trace: bool,
    /// Whether to compile everything twice and compare. Overridden by `RK_TWICE`.
    #[serde(default)]
    pub twice: bool,
    /// The bring-up classes to delegate, as in `m16` or `as`. Overridden by `RK_BRINGUP`, which
    /// takes a comma separated list. Empty in every graded run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bringup: Vec<String>,
    /// The compiler a delegated call goes to. Overridden by `RK_BRINGUP_CC`.
    #[serde(default, skip_serializing_if = "crate::config::is_empty_path")]
    pub bringup_cc: PathBuf,
}

/// Whether a path is empty, for serde.
#[must_use]
pub fn is_empty_path(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

impl ShimConfig {
    /// Read the file next to the shim, if any, then apply the environment.
    ///
    /// Returns `None` when neither says which compiler to run, which is the one setting without
    /// a sensible default.
    #[must_use]
    pub fn load(shim_dir: Option<&Path>, env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let mut config = shim_dir
            .map(|dir| dir.join(FILE_NAME))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| toml::from_str::<Self>(&text).ok())
            .unwrap_or_default();
        if let Some(real) = env("RK_REAL_CC").filter(|v| !v.is_empty()) {
            config.real = PathBuf::from(real);
        }
        if let Some(log) = env("RK_COMPILE_LOG").filter(|v| !v.is_empty()) {
            config.log = PathBuf::from(log);
        }
        if let Some(value) = env("RK_RUCC_TRACE") {
            config.rucc_trace = truthy(&value);
        }
        if let Some(value) = env("RK_TWICE") {
            config.twice = truthy(&value);
        }
        if let Some(value) = env("RK_BRINGUP") {
            config.bringup = value
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(String::from)
                .collect();
        }
        if let Some(cc) = env("RK_BRINGUP_CC").filter(|v| !v.is_empty()) {
            config.bringup_cc = PathBuf::from(cc);
        }
        (!config.real.as_os_str().is_empty()).then_some(config)
    }

    /// The file's text.
    #[must_use]
    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }
}

fn truthy(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

/// The environment variables that can change what a compiler does, recorded when set.
///
/// `PATH` is here because it decides which assembler and linker GCC finds. The locale variables
/// are here because they change the language of diagnostics, which kbuild probes sometimes grep.
/// `KBUILD_` and `RUCC_` variables are recorded as well.
pub const RECORDED_ENV: &[&str] = &[
    "PATH",
    "CPATH",
    "C_INCLUDE_PATH",
    "LIBRARY_PATH",
    "COMPILER_PATH",
    "GCC_EXEC_PREFIX",
    "SOURCE_DATE_EPOCH",
    "DEPENDENCIES_OUTPUT",
    "SUNPRO_DEPENDENCIES",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "ARCH",
    "CROSS_COMPILE",
    "LLVM",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn with_nothing_to_go_on_there_is_no_config() {
        assert!(ShimConfig::load(None, &env_of(&[])).is_none());
    }

    #[test]
    fn the_environment_is_enough_on_its_own() {
        let config = ShimConfig::load(
            None,
            &env_of(&[("RK_REAL_CC", "/usr/bin/gcc-16"), ("RK_TWICE", "1")]),
        )
        .unwrap();
        assert_eq!(config.real, Path::new("/usr/bin/gcc-16"));
        assert!(config.twice);
        assert!(!config.rucc_trace);
    }

    #[test]
    fn the_file_is_read_and_the_environment_wins() {
        let dir = std::env::temp_dir().join(format!("rk-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let written = ShimConfig {
            real: "/opt/rucc".into(),
            log: "/b/compile.jsonl".into(),
            rucc_trace: true,
            twice: false,
            bringup: vec![],
            bringup_cc: PathBuf::new(),
        };
        std::fs::write(dir.join(FILE_NAME), written.to_toml()).unwrap();
        let read = ShimConfig::load(Some(&dir), &env_of(&[])).unwrap();
        assert_eq!(read, written);
        let overridden = ShimConfig::load(Some(&dir), &env_of(&[("RK_RUCC_TRACE", "0")])).unwrap();
        assert!(!overridden.rucc_trace);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bringup_is_a_comma_separated_list() {
        let config = ShimConfig::load(
            None,
            &env_of(&[
                ("RK_REAL_CC", "/opt/rucc"),
                ("RK_BRINGUP", "m16, as"),
                ("RK_BRINGUP_CC", "/usr/bin/gcc"),
            ]),
        )
        .unwrap();
        assert_eq!(config.bringup, ["m16", "as"]);
        assert_eq!(config.bringup_cc, Path::new("/usr/bin/gcc"));
    }
}
