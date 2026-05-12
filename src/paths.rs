//! Resolved filesystem paths for the daemon's runtime state.
//!
//! All paths derive from a single base directory. By default this is
//! `~/.config/scribed` on Linux and `~/Library/Application Support/scribed` on
//! macOS (via [`directories::ProjectDirs`]). The `SCRIBED_CONFIG_DIR`
//! environment variable overrides the base, which is what tests rely on.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

/// Environment variable that overrides the config base directory.
pub const ENV_CONFIG_DIR: &str = "SCRIBED_CONFIG_DIR";

/// Resolved paths owned by the daemon. Construct once at startup.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub pid_file: PathBuf,
    pub control_socket: PathBuf,
    pub log_file: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    /// Resolve paths from the environment. Honors `SCRIBED_CONFIG_DIR`.
    pub fn from_env() -> Self {
        let base = std::env::var_os(ENV_CONFIG_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(default_config_dir);
        let cache = default_cache_dir();
        Self::from_base(&base, &cache)
    }

    /// Resolve paths from explicit roots. Used by tests with `tempfile`.
    pub fn from_base(config_dir: &Path, cache_dir: &Path) -> Self {
        Self {
            config_dir: config_dir.to_path_buf(),
            config_file: config_dir.join("config.toml"),
            pid_file: config_dir.join("daemon.pid"),
            control_socket: config_dir.join("daemon.sock"),
            log_file: config_dir.join("daemon.log"),
            cache_dir: cache_dir.to_path_buf(),
        }
    }
}

fn default_config_dir() -> PathBuf {
    if let Some(dirs) = ProjectDirs::from("dev", "", "scribed") {
        return dirs.config_dir().to_path_buf();
    }
    // Fallback if ProjectDirs can't resolve (no $HOME, etc.)
    PathBuf::from(".scribed")
}

fn default_cache_dir() -> PathBuf {
    if let Some(dirs) = ProjectDirs::from("dev", "", "scribed") {
        return dirs.cache_dir().to_path_buf();
    }
    PathBuf::from(".scribed-cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_base_lays_out_files_under_config_dir() {
        let base = PathBuf::from("/tmp/scribed-test");
        let cache = PathBuf::from("/tmp/scribed-cache");
        let p = Paths::from_base(&base, &cache);
        assert_eq!(p.config_file, base.join("config.toml"));
        assert_eq!(p.pid_file, base.join("daemon.pid"));
        assert_eq!(p.control_socket, base.join("daemon.sock"));
        assert_eq!(p.log_file, base.join("daemon.log"));
        assert_eq!(p.cache_dir, cache);
    }

    #[test]
    fn env_override_changes_base() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os(ENV_CONFIG_DIR);
        std::env::set_var(ENV_CONFIG_DIR, tmp.path());
        let p = Paths::from_env();
        assert_eq!(p.config_dir, tmp.path());
        match prev {
            Some(v) => std::env::set_var(ENV_CONFIG_DIR, v),
            None => std::env::remove_var(ENV_CONFIG_DIR),
        }
    }
}
