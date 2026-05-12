//! The IPC protocol — typed messages between the CLI and the daemon.
//!
//! Wire format: one JSON object per line. The CLI sends a [`DaemonCommand`],
//! the daemon replies with a [`DaemonReply`]. Lines are newline-terminated.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum DaemonCommand {
    /// Return the daemon's runtime status.
    Status,
    /// Initiate a graceful shutdown.
    Stop,
    /// Toggle recording on/off (equivalent to pressing the hotkey).
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum DaemonReply {
    Status {
        pid: i32,
        recording: bool,
        hotkey: String,
        mode: String,
    },
    Ok,
    Error {
        message: String,
    },
}

impl DaemonCommand {
    pub fn to_wire(&self) -> String {
        let mut s = serde_json::to_string(self).expect("serialize daemon command");
        s.push('\n');
        s
    }

    pub fn from_wire(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line.trim())
    }
}

impl DaemonReply {
    pub fn to_wire(&self) -> String {
        let mut s = serde_json::to_string(self).expect("serialize daemon reply");
        s.push('\n');
        s
    }

    pub fn from_wire(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_round_trip_status() {
        let cmd = DaemonCommand::Status;
        assert_eq!(DaemonCommand::from_wire(&cmd.to_wire()).unwrap(), cmd);
    }

    #[test]
    fn command_round_trip_stop() {
        let cmd = DaemonCommand::Stop;
        assert_eq!(DaemonCommand::from_wire(&cmd.to_wire()).unwrap(), cmd);
    }

    #[test]
    fn command_round_trip_toggle() {
        let cmd = DaemonCommand::Toggle;
        assert_eq!(DaemonCommand::from_wire(&cmd.to_wire()).unwrap(), cmd);
    }

    #[test]
    fn reply_round_trip_status() {
        let r = DaemonReply::Status {
            pid: 4242,
            recording: false,
            hotkey: "ctrl+shift+space".into(),
            mode: "toggle".into(),
        };
        assert_eq!(DaemonReply::from_wire(&r.to_wire()).unwrap(), r);
    }

    #[test]
    fn reply_round_trip_ok() {
        let r = DaemonReply::Ok;
        assert_eq!(DaemonReply::from_wire(&r.to_wire()).unwrap(), r);
    }

    #[test]
    fn reply_round_trip_error() {
        let r = DaemonReply::Error {
            message: "oops".into(),
        };
        assert_eq!(DaemonReply::from_wire(&r.to_wire()).unwrap(), r);
    }

    #[test]
    fn wire_format_is_line_delimited() {
        let s = DaemonCommand::Stop.to_wire();
        assert!(s.ends_with('\n'));
        assert_eq!(s.matches('\n').count(), 1);
    }
}
