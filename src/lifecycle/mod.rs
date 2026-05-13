//! Lifecycle bounded context — daemon spawn, PID file, control socket,
//! shutdown signals.
//!
//! The fork-vs-Tokio invariant: [`Daemonize::start()`] *must* run before any
//! Tokio call. Tokio's reactor uses an inherited file descriptor that does
//! not survive `fork()`. We enforce this by gating the runtime construction
//! on a function that only runs after the optional daemonize step.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::Notify;

use crate::config::Config;
use crate::paths::Paths;

pub mod ipc;
pub mod liveness;
pub mod pidfile;
pub mod protocol;

use crate::lifecycle::pidfile::PidRecord;
use crate::lifecycle::protocol::{DaemonCommand, DaemonReply};

#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("pid file: {0}")]
    Pid(String),
    #[error("daemon already running (pid {pid})")]
    AlreadyRunning { pid: i32 },
    #[error("daemon not running")]
    NotRunning,
    #[error("ipc: {0}")]
    Ipc(#[from] ipc::IpcError),
    #[error("daemonize: {0}")]
    Daemonize(String),
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}

/// `scribed start [--background]`.
pub fn start(paths: &Paths, config: &Config, background: bool) -> crate::Result<()> {
    if let Some(record) = pidfile::read(&paths.pid_file)? {
        if liveness::classify(record.pid) == liveness::Liveness::Alive {
            return Err(LifecycleError::AlreadyRunning { pid: record.pid }.into());
        }
        tracing::warn!(pid = record.pid, "removing stale pid file");
        pidfile::remove(&paths.pid_file)?;
    }

    // Surface environment issues to the terminal before we fork into the
    // background and the user loses sight of stderr.
    preflight_warnings(config);

    if background {
        background_spawn(paths)?;
    } else {
        run_foreground(paths, config)?;
    }
    Ok(())
}

/// Prints user-visible warnings to stderr for environment problems that won't
/// stop the daemon from starting, but will degrade behavior. Called from
/// [`start`] before the optional fork so the messages reach the user's
/// terminal, not the daemon log.
fn preflight_warnings(config: &Config) {
    use crate::config::OutputMode;
    use crate::output::backend;

    if backend::is_wayland() {
        let injection_intended =
            matches!(config.output_mode, OutputMode::Auto | OutputMode::Injection);
        let resolved = backend::select_backend_kind();
        if injection_intended && resolved != backend::BackendKind::Ydotool {
            let reason = if !backend::ydotool_available() {
                "`ydotool` binary not found on PATH"
            } else {
                "`ydotoold` socket not found (daemon not running)"
            };
            eprintln!();
            eprintln!("warning: Wayland session detected, but {reason}.");
            eprintln!(
                "         Dictation will fall back to the {} backend; keystroke",
                resolved.as_str()
            );
            eprintln!("         injection into the focused window will not work.");
            eprintln!("         To enable injection:");
            eprintln!("           systemctl --user enable --now ydotoold");
            eprintln!();
        }
    }
}

/// `scribed stop`.
pub fn stop(paths: &Paths) -> crate::Result<()> {
    let record = pidfile::read(&paths.pid_file)?.ok_or(LifecycleError::NotRunning)?;
    if liveness::classify(record.pid) != liveness::Liveness::Alive {
        pidfile::remove(&paths.pid_file)?;
        return Err(LifecycleError::NotRunning.into());
    }

    // Prefer IPC; fall back to SIGTERM if the socket isn't reachable.
    let socket = paths.control_socket.clone();
    match ipc::client::send(&socket, DaemonCommand::Stop) {
        Ok(_) => {
            wait_for_exit(record.pid, Duration::from_secs(5));
        }
        Err(e) => {
            tracing::warn!(?e, "IPC stop failed; falling back to SIGTERM");
            sigterm_with_grace(record.pid, Duration::from_secs(5))?;
        }
    }
    pidfile::remove(&paths.pid_file)?;
    println!("scribed: stopped (pid {})", record.pid);
    Ok(())
}

/// `scribed status`.
pub fn status(paths: &Paths, config: &Config) -> crate::Result<()> {
    println!("scribed status");
    println!("  config dir : {}", paths.config_dir.display());
    println!("  config file: {}", paths.config_file.display());
    println!("  pid file   : {}", paths.pid_file.display());
    println!("  socket     : {}", paths.control_socket.display());
    println!("  hotkey     : {}", config.hotkey);
    println!("  mode       : {:?}", config.mode);
    println!("  model      : {}", config.model);
    println!(
        "  output     : {}",
        crate::output::backend::select_backend_kind().as_str()
    );

    match pidfile::read(&paths.pid_file) {
        Ok(Some(record)) => match liveness::classify(record.pid) {
            liveness::Liveness::Alive => {
                println!("  daemon     : running (pid {})", record.pid);
                if let Ok(DaemonReply::Status { recording, .. }) =
                    ipc::client::send(&paths.control_socket, DaemonCommand::Status)
                {
                    println!("  recording  : {recording}");
                }
            }
            liveness::Liveness::Stale => {
                println!(
                    "  daemon     : stale pid file (pid {} not alive)",
                    record.pid
                );
            }
        },
        Ok(None) => println!("  daemon     : not running"),
        Err(e) => println!("  daemon     : pid file unreadable ({e})"),
    }
    Ok(())
}

/// `scribed run`. The daemon's main loop. Called directly when running in the
/// foreground, or by the spawned child after `--background` forks.
pub fn run_foreground(paths: &Paths, config: &Config) -> crate::Result<()> {
    let record = PidRecord {
        pid: std::process::id() as i32,
        command: "scribed run".into(),
        created_at: Utc::now(),
        config_dir: paths.config_dir.display().to_string(),
    };
    pidfile::write(&paths.pid_file, &record).map_err(|e| LifecycleError::Pid(e.to_string()))?;

    let result = run_loop(paths, config);

    pidfile::remove(&paths.pid_file).ok();
    if paths.control_socket.exists() {
        let _ = std::fs::remove_file(&paths.control_socket);
    }
    result
}

/// `scribed toggle`.
pub fn toggle(paths: &Paths) -> crate::Result<()> {
    let record = pidfile::read(&paths.pid_file)?.ok_or(LifecycleError::NotRunning)?;
    if liveness::classify(record.pid) != liveness::Liveness::Alive {
        return Err(LifecycleError::NotRunning.into());
    }
    let reply = ipc::client::send(&paths.control_socket, DaemonCommand::Toggle)
        .map_err(LifecycleError::from)?;
    match reply {
        DaemonReply::Ok => {
            println!("scribed: toggled");
            Ok(())
        }
        DaemonReply::Error { message } => {
            Err(LifecycleError::Ipc(ipc::IpcError::DaemonError(message)).into())
        }
        other => {
            tracing::warn!(?other, "unexpected reply to toggle");
            Ok(())
        }
    }
}

fn background_spawn(paths: &Paths) -> crate::Result<()> {
    // Spawn `scribed run` as a detached child. We deliberately do NOT use
    // daemonize::Daemonize::start() here because the parent CLI uses a tokio
    // runtime for the IPC client; forking with a live runtime is unsafe. The
    // simpler pattern: fork-exec via std::process::Command with a new session.
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    let log_path = paths.log_file.clone();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let log_err = log.try_clone()?;

    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--config-dir").arg(&paths.config_dir);
    cmd.arg("run");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(log);
    cmd.stderr(log_err);
    unsafe {
        cmd.pre_exec(|| {
            // Detach from controlling terminal.
            nix::unistd::setsid().map_err(|e| std::io::Error::other(format!("setsid: {e}")))?;
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    let child_pid = child.id() as i32;
    drop(child);
    tracing::info!(pid = child_pid, "spawned background daemon");

    // Poll the pid file briefly to confirm the child wrote its identity.
    // Model load takes a few seconds, so we give it more headroom than the
    // pure-IPC daemon needed.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = pidfile::read(&paths.pid_file) {
            println!("scribed: started (pid {child_pid})");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(LifecycleError::Pid(format!(
        "background daemon did not write pid file at {}",
        paths.pid_file.display()
    ))
    .into())
}

/// The daemon's main async loop. Owns the control socket and waits for either
/// a `Stop` command, a SIGTERM/SIGINT, or an internal failure.
fn run_loop(paths: &Paths, config: &Config) -> crate::Result<()> {
    #[cfg(feature = "asr")]
    let runtime: Option<Arc<Mutex<crate::service::Runtime>>> = {
        let model_dir = paths
            .cache_dir
            .join("sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8");
        match crate::service::Runtime::load(config, model_dir.clone()) {
            Ok(rt) => Some(Arc::new(Mutex::new(rt))),
            Err(e) => {
                tracing::warn!(
                    ?e,
                    dir = %model_dir.display(),
                    "ASR runtime disabled — hotkey will toggle state but no transcription will run. Run `scribed fetch-model` to enable."
                );
                None
            }
        }
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let shutdown = Arc::new(Notify::new());
        let state = Arc::new(Mutex::new(DaemonState {
            recording: false,
            hotkey: config.hotkey.clone(),
            mode: format!("{:?}", config.mode).to_lowercase(),
        }));
        let handler = Arc::new(IpcHandler {
            state: state.clone(),
            shutdown: shutdown.clone(),
            #[cfg(feature = "asr")]
            runtime: runtime.clone(),
        });

        let listener = ipc::server::bind(&paths.control_socket)
            .await
            .map_err(LifecycleError::from)?;
        tracing::info!(socket = %paths.control_socket.display(), "daemon listening");

        let socket_path = paths.control_socket.clone();
        let accept_handler = handler.clone();
        let accept_task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let h = accept_handler.clone();
                        tokio::spawn(async move {
                            ipc::server::serve_one(stream, h).await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!(?e, "accept error");
                        break;
                    }
                }
            }
        });

        let _hotkey_listener = start_hotkey_listener(
            config,
            state.clone(),
            #[cfg(feature = "asr")]
            runtime.clone(),
        );

        let sigint = tokio::signal::ctrl_c();
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        tokio::select! {
            _ = shutdown.notified() => {
                tracing::info!("shutdown via IPC");
            }
            _ = sigint => {
                tracing::info!("shutdown via SIGINT");
            }
            _ = sigterm.recv() => {
                tracing::info!("shutdown via SIGTERM");
            }
        }
        accept_task.abort();
        let _ = std::fs::remove_file(&socket_path);
        Ok::<_, crate::Error>(())
    })?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn start_hotkey_listener(
    config: &Config,
    state: Arc<Mutex<DaemonState>>,
    #[cfg(feature = "asr")] runtime: Option<Arc<Mutex<crate::service::Runtime>>>,
) -> Option<crate::input::evdev_listener::EvdevListener> {
    use crate::input::evdev_listener::EvdevListener;
    use crate::input::{KeyChord, RecordingIntent};

    let chord = match KeyChord::parse(&config.hotkey) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(hotkey = %config.hotkey, ?e, "invalid hotkey; listener disabled");
            return None;
        }
    };
    let chord_display = chord.to_string();
    match EvdevListener::start(chord, config.mode, move |intent| {
        let recording = match intent {
            RecordingIntent::Start => true,
            RecordingIntent::Stop => false,
            RecordingIntent::Toggle => !state.lock().recording,
        };
        state.lock().recording = recording;
        tracing::info!(?intent, recording, "hotkey");
        #[cfg(feature = "asr")]
        if let Some(rt) = &runtime {
            let mut rt = rt.lock();
            if recording {
                rt.start_session();
            } else {
                rt.stop_session();
            }
        }
    }) {
        Ok(listener) => {
            tracing::info!(hotkey = %chord_display, "hotkey listener active");
            Some(listener)
        }
        Err(e) => {
            tracing::warn!(?e, "hotkey listener failed to start; daemon will run without global hotkey");
            None
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn start_hotkey_listener(
    _config: &Config,
    _state: Arc<Mutex<DaemonState>>,
    #[cfg(feature = "asr")] _runtime: Option<Arc<Mutex<crate::service::Runtime>>>,
) -> Option<()> {
    tracing::warn!("hotkey listener not yet implemented on this platform");
    None
}

#[derive(Debug)]
struct DaemonState {
    recording: bool,
    hotkey: String,
    mode: String,
}

struct IpcHandler {
    state: Arc<Mutex<DaemonState>>,
    shutdown: Arc<Notify>,
    #[cfg(feature = "asr")]
    runtime: Option<Arc<Mutex<crate::service::Runtime>>>,
}

impl ipc::server::CommandHandler for IpcHandler {
    fn handle(
        &self,
        cmd: DaemonCommand,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DaemonReply> + Send + '_>> {
        Box::pin(async move {
            match cmd {
                DaemonCommand::Status => {
                    let s = self.state.lock();
                    DaemonReply::Status {
                        pid: std::process::id() as i32,
                        recording: s.recording,
                        hotkey: s.hotkey.clone(),
                        mode: s.mode.clone(),
                    }
                }
                DaemonCommand::Stop => {
                    self.shutdown.notify_waiters();
                    DaemonReply::Ok
                }
                DaemonCommand::Toggle => {
                    let recording = {
                        let mut s = self.state.lock();
                        s.recording = !s.recording;
                        s.recording
                    };
                    #[cfg(feature = "asr")]
                    if let Some(rt) = &self.runtime {
                        let mut rt = rt.lock();
                        if recording {
                            rt.start_session();
                        } else {
                            rt.stop_session();
                        }
                    }
                    let _ = recording;
                    DaemonReply::Ok
                }
            }
        })
    }
}

fn wait_for_exit(pid: i32, deadline: Duration) {
    let until = Instant::now() + deadline;
    while Instant::now() < until {
        if !liveness::process_exists(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sigterm_with_grace(pid: i32, grace: Duration) -> Result<(), LifecycleError> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
    wait_for_exit(pid, grace);
    if liveness::process_exists(pid) {
        tracing::warn!("daemon did not exit on SIGTERM; sending SIGKILL");
        let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        wait_for_exit(pid, Duration::from_secs(2));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn status_without_daemon_reports_not_running() {
        let dir = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = Paths::from_base(dir.path(), cache.path());
        let cfg = Config::default();
        // Should not error
        status(&paths, &cfg).unwrap();
    }

    #[test]
    fn stop_without_daemon_returns_not_running() {
        let dir = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = Paths::from_base(dir.path(), cache.path());
        let err = stop(&paths).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not running"), "got {msg}");
    }
}
