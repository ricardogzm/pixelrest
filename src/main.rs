mod app;
mod brightness;
mod config;
mod ipc;
mod media;
mod tray;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// OLED burn-in protection for KDE Plasma on Wayland.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon (default)
    Run,
    /// Toggle pause on the running daemon
    TogglePause,
    /// Pause for MINUTES (default: pause_minutes from the config)
    Pause { minutes: Option<u32> },
    /// Resume after a pause
    Resume,
    /// Blank the protected monitors now
    Blank,
    /// Dim the protected monitors now
    Dim,
    /// Undo a blank/dim
    Wake,
    /// Print the daemon's state
    Status,
    /// Reload the config file
    Reload,
    /// Stop the daemon
    Quit,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,pixelrest=info"))
        .format_timestamp_secs()
        .init();
    if let Err(e) = real_main() {
        log::error!("{e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let (method, minutes) = match Cli::parse().command.unwrap_or(Command::Run) {
        Command::Run => return app::run(config::Config::load()?),
        Command::TogglePause => ("TogglePause", None),
        Command::Pause { minutes } => ("Pause", Some(minutes.unwrap_or(0))),
        Command::Resume => ("Resume", None),
        Command::Blank => ("Blank", None),
        Command::Dim => ("Dim", None),
        Command::Wake => ("Wake", None),
        Command::Status => ("Status", None),
        Command::Reload => ("Reload", None),
        Command::Quit => ("Quit", None),
    };
    if let Some(reply) = ipc::call(method, minutes)? {
        println!("{reply}");
    }
    Ok(())
}
