//! The PID file is the source of truth for "is the daemon running?". It lives
//! at `~/.config/scribed/daemon.pid` and is written atomically via
//! `tempfile::NamedTempFile::persist`.
//!
//! Phase 1 ships read + write; the liveness check (`kill -0` + cmdline match)
//! and atomic-rotate-on-startup lands in Phase 4.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What we write into the PID file. JSON for forward compatibility — adding a
/// new field doesn't break old readers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PidRecord {
    pub pid: i32,
    pub command: String,
    pub created_at: DateTime<Utc>,
    pub config_dir: String,
}

/// Write `record` to `path` atomically. Creates the parent directory if missing.
pub fn write(path: &Path, record: &PidRecord) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let tmp = tempfile::NamedTempFile::new_in(parent)?;
    let json = serde_json::to_string(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(tmp.path(), json)?;
    tmp.persist(path).map_err(std::io::Error::other)?;
    Ok(())
}

/// Read the PID file. Returns `Ok(None)` if the file is absent.
pub fn read(path: &Path) -> std::io::Result<Option<PidRecord>> {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<PidRecord>(&s) {
            Ok(record) => Ok(Some(record)),
            Err(_) => {
                // Try legacy: bare pid integer.
                if let Ok(pid) = s.trim().parse::<i32>() {
                    return Ok(Some(PidRecord {
                        pid,
                        command: String::new(),
                        created_at: Utc::now(),
                        config_dir: String::new(),
                    }));
                }
                Ok(None)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Remove the PID file. No error if absent.
pub fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample() -> PidRecord {
        PidRecord {
            pid: 4242,
            command: "scribed run".into(),
            created_at: Utc::now(),
            config_dir: "/tmp/scribed".into(),
        }
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("daemon.pid");
        let record = sample();
        write(&path, &record).unwrap();
        let read = read(&path).unwrap().unwrap();
        assert_eq!(read.pid, record.pid);
        assert_eq!(read.command, record.command);
        assert_eq!(read.config_dir, record.config_dir);
    }

    #[test]
    fn read_missing_file_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nothing.pid");
        assert!(read(&path).unwrap().is_none());
    }

    #[test]
    fn read_legacy_bare_pid() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("daemon.pid");
        std::fs::write(&path, "9999").unwrap();
        let record = read(&path).unwrap().unwrap();
        assert_eq!(record.pid, 9999);
    }

    #[test]
    fn write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("nested/sub/daemon.pid");
        write(&nested, &sample()).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn remove_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("daemon.pid");
        remove(&path).unwrap();
        write(&path, &sample()).unwrap();
        remove(&path).unwrap();
        assert!(!path.exists());
        remove(&path).unwrap();
    }
}
