//! Where things are: the repository, the cache, and the default build directories.
//!
//! `rk` is run from anywhere inside the repository and finds its root by walking up to the
//! directory holding `pins.toml`, the way cargo finds a manifest. `RK_ROOT` overrides the walk.

use std::path::{Path, PathBuf};

/// The repository root and the paths hanging off it.
#[derive(Debug, Clone)]
pub struct Repo {
    /// The directory holding `pins.toml`.
    pub root: PathBuf,
}

impl Repo {
    /// Find the repository from `RK_ROOT` or by walking up from the working directory.
    pub fn find() -> Result<Self, String> {
        if let Ok(root) = std::env::var("RK_ROOT") {
            let root = PathBuf::from(root);
            if root.join("pins.toml").is_file() {
                return Ok(Self { root });
            }
            return Err(format!("RK_ROOT={} has no pins.toml", root.display()));
        }
        let start = std::env::current_dir().map_err(|e| format!("no working directory: {e}"))?;
        let mut here = start.as_path();
        loop {
            if here.join("pins.toml").is_file() {
                return Ok(Self {
                    root: here.to_path_buf(),
                });
            }
            here = here.parent().ok_or_else(|| {
                format!(
                    "{} is not inside a rucc-kernel checkout (no pins.toml above it); set RK_ROOT",
                    start.display()
                )
            })?;
        }
    }

    /// A file at the root of the repository.
    #[must_use]
    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

/// The download and source cache: `RK_CACHE`, or `~/.cache/rk`.
#[must_use]
pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RK_CACHE")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".cache").join("rk")
}
