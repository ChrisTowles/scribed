//! Command-line interface. Parses arguments and dispatches to the
//! [Lifecycle] context.
//!
//! [Lifecycle]: ../DOMAIN.md

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::paths::Paths;
use crate::Result;

#[derive(Debug, Parser)]
#[command(
    name = "scribed",
    version,
    about = "Local streaming dictation daemon",
    long_about = None,
)]
pub struct Cli {
    /// Override the config base directory (default: ~/.config/scribed).
    #[arg(long, global = true, env = "SCRIBED_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,

    /// Log level. Honors `RUST_LOG` when omitted.
    #[arg(long, global = true)]
    pub log_level: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the daemon.
    Start {
        /// Detach into the background. Without this flag the daemon runs in
        /// the foreground and dies with the terminal.
        #[arg(long)]
        background: bool,
    },
    /// Stop the running daemon.
    Stop,
    /// Print the daemon's current status.
    Status,
    /// Run the daemon in the foreground (used internally by --background).
    Run,
    /// Toggle recording (equivalent to pressing the hotkey).
    Toggle,
    /// Print the resolved config (after sanitization) and exit.
    PrintConfig,
    /// Download the ASR model bundle to the local cache and exit.
    FetchModel,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.log_level.as_deref());

    let paths = match cli.config_dir {
        Some(ref dir) => Paths::from_base(dir, &crate::paths::default_cache_dir()),
        None => Paths::from_env(),
    };

    let config = Config::load(&paths.config_file)?;

    match cli.command {
        Command::Start { background } => crate::lifecycle::start(&paths, &config, background)?,
        Command::Stop => crate::lifecycle::stop(&paths)?,
        Command::Status => crate::lifecycle::status(&paths, &config)?,
        Command::Run => crate::lifecycle::run_foreground(&paths, &config)?,
        Command::Toggle => crate::lifecycle::toggle(&paths)?,
        Command::PrintConfig => {
            let toml = config.to_toml()?;
            println!("{toml}");
        }
        Command::FetchModel => {
            let target = crate::asr::download::ensure(
                &crate::asr::download::STREAMING_MODEL,
                &paths.cache_dir,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("Model ready: {}", target.display());
        }
    }
    Ok(())
}

fn init_logging(explicit: Option<&str>) {
    let filter = match explicit {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_parses_subcommands() {
        Cli::command().debug_assert();
    }

    #[test]
    fn start_accepts_background_flag() {
        let cli = Cli::try_parse_from(["scribed", "start", "--background"]).unwrap();
        assert!(matches!(cli.command, Command::Start { background: true }));
    }

    #[test]
    fn start_without_background() {
        let cli = Cli::try_parse_from(["scribed", "start"]).unwrap();
        assert!(matches!(cli.command, Command::Start { background: false }));
    }

    #[test]
    fn config_dir_global_flag() {
        let cli = Cli::try_parse_from(["scribed", "--config-dir", "/tmp/x", "status"]).unwrap();
        assert_eq!(
            cli.config_dir.as_deref(),
            Some(std::path::Path::new("/tmp/x"))
        );
    }
}
