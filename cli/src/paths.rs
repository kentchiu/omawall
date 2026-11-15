use std::env;
use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

/// Every filesystem location the CLI reads or writes.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub favorites_dir: PathBuf,
    pub theme_colors: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| AppError::new("io", "HOME directory not found"))?;
        let config = xdg("XDG_CONFIG_HOME", &home, ".config");
        let state = xdg("XDG_STATE_HOME", &home, ".local/state");
        let cache = xdg("XDG_CACHE_HOME", &home, ".cache");
        Ok(Self::new(&home, &config, &state, &cache))
    }

    pub fn new(home: &Path, config: &Path, state: &Path, cache: &Path) -> Self {
        Self {
            config_file: config.join("omawall/config.json"),
            state_dir: state.join("omawall"),
            cache_dir: cache.join("omawall"),
            favorites_dir: home.join("Pictures/Wallpapers/wallhaven"),
            theme_colors: state.join("omarchy/current/theme/colors.toml"),
        }
    }

    pub fn state(&self, name: &str) -> PathBuf {
        self.state_dir.join(name)
    }

    pub fn full_dir(&self) -> PathBuf {
        self.cache_dir.join("full")
    }

    pub fn thumbs_dir(&self) -> PathBuf {
        self.cache_dir.join("thumbs")
    }

    /// Saved search pages, one folder per query key.
    pub fn pages_dir(&self) -> PathBuf {
        self.cache_dir.join("pages")
    }
}

/// XDG base dir: the variable when it holds an absolute path, else `$HOME/<fallback>`.
fn xdg(var: &str, home: &Path, fallback: &str) -> PathBuf {
    env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
}
