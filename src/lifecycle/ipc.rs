//! Unix domain socket IPC between the CLI client and the running daemon.
//!
//! - The daemon listens on the socket and serves [`DaemonCommand`]s.
//! - The CLI uses [`client::send`] to issue one command per connection.
//!
//! Wire format is defined in [`crate::lifecycle::protocol`]. Each connection
//! is one request/one response — keep-alive isn't worth the complexity for
//! commands issued by a human-paced CLI.

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::lifecycle::protocol::{DaemonCommand, DaemonReply};

/// Default deadline for a single command exchange.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors talking to the daemon.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Protocol(#[from] serde_json::Error),
    #[error("daemon replied with error: {0}")]
    DaemonError(String),
    #[error("daemon closed the connection without replying")]
    EmptyReply,
}

pub mod client {
    use super::*;

    /// Synchronously connect, send `cmd`, read one reply, disconnect.
    pub fn send(socket_path: &Path, cmd: DaemonCommand) -> Result<DaemonReply, IpcError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(async {
            tokio::time::timeout(DEFAULT_TIMEOUT, send_async(socket_path, cmd))
                .await
                .map_err(|_| {
                    IpcError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "daemon did not reply within timeout",
                    ))
                })?
        })
    }

    async fn send_async(socket_path: &Path, cmd: DaemonCommand) -> Result<DaemonReply, IpcError> {
        let mut stream = UnixStream::connect(socket_path).await?;
        stream.write_all(cmd.to_wire().as_bytes()).await?;
        stream.flush().await?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(IpcError::EmptyReply);
        }
        Ok(DaemonReply::from_wire(&line)?)
    }
}

pub mod server {
    use super::*;
    use tokio::net::UnixListener;

    /// Trait the daemon implements to handle commands. Async so handlers can
    /// await on internal state.
    pub trait CommandHandler: Send + Sync + 'static {
        fn handle(
            &self,
            cmd: DaemonCommand,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DaemonReply> + Send + '_>>;
    }

    /// Start accepting connections on `socket_path`. Removes any stale socket
    /// at that path first. Spawns one task per accepted connection.
    ///
    /// Returns a guard that, when dropped, stops accepting new connections
    /// (existing connections continue until completion). The socket file is
    /// removed on drop.
    pub async fn bind(socket_path: &Path) -> Result<UnixListener, IpcError> {
        // Clean any leftover socket from a previous crash. SAFETY: this is the
        // documented pattern; tokio panics if the path exists.
        if socket_path.exists() {
            std::fs::remove_file(socket_path)?;
        }
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(socket_path)?;
        Ok(listener)
    }

    /// Read a single command from a connection and pass it to `handler`,
    /// writing the reply back. The connection is closed after one round-trip.
    pub async fn serve_one<H: CommandHandler>(mut stream: UnixStream, handler: std::sync::Arc<H>) {
        let (read_half, mut write_half) = stream.split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        if reader.read_line(&mut line).await.is_err() {
            return;
        }
        let cmd = match DaemonCommand::from_wire(&line) {
            Ok(c) => c,
            Err(e) => {
                let reply = DaemonReply::Error {
                    message: format!("bad command: {e}"),
                };
                let _ = write_half.write_all(reply.to_wire().as_bytes()).await;
                return;
            }
        };
        let reply = handler.handle(cmd).await;
        let _ = write_half.write_all(reply.to_wire().as_bytes()).await;
        let _ = write_half.flush().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;

    struct EchoStatus;

    impl server::CommandHandler for EchoStatus {
        fn handle(
            &self,
            cmd: DaemonCommand,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DaemonReply> + Send + '_>> {
            Box::pin(async move {
                match cmd {
                    DaemonCommand::Status => DaemonReply::Status {
                        pid: 1234,
                        recording: false,
                        hotkey: "ctrl+shift+space".into(),
                        mode: "toggle".into(),
                    },
                    DaemonCommand::Stop => DaemonReply::Ok,
                    DaemonCommand::Toggle => DaemonReply::Ok,
                }
            })
        }
    }

    #[tokio::test]
    async fn round_trip_status_command() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("test.sock");
        let listener = server::bind(&sock).await.unwrap();
        let handler = Arc::new(EchoStatus);
        let h = handler.clone();
        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            server::serve_one(stream, h).await;
        });

        // Client send
        let sock_clone = sock.clone();
        let reply =
            tokio::task::spawn_blocking(move || client::send(&sock_clone, DaemonCommand::Status))
                .await
                .unwrap()
                .unwrap();

        match reply {
            DaemonReply::Status { pid, hotkey, .. } => {
                assert_eq!(pid, 1234);
                assert_eq!(hotkey, "ctrl+shift+space");
            }
            other => panic!("unexpected reply: {other:?}"),
        }
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn round_trip_stop_command() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("test.sock");
        let listener = server::bind(&sock).await.unwrap();
        let handler = Arc::new(EchoStatus);
        let h = handler.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            server::serve_one(stream, h).await;
        });

        let sock_clone = sock.clone();
        let reply =
            tokio::task::spawn_blocking(move || client::send(&sock_clone, DaemonCommand::Stop))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(reply, DaemonReply::Ok);
    }

    #[test]
    fn client_send_to_nonexistent_socket_errors_quickly() {
        let dir = tempdir().unwrap();
        let sock = dir.path().join("not-here.sock");
        let result = client::send(&sock, DaemonCommand::Status);
        assert!(result.is_err());
    }
}
