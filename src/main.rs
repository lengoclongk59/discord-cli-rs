//! discord-cli — read-only Discord archival CLI built on `discord-user-rs`.

mod api;
mod auth;
mod cli;
mod commands;
mod config;
mod db;
mod ids;
mod output;
mod types;
mod wire_enums;

use clap::Parser;
use cli::Cli;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.no_color || std::env::var("NO_COLOR").is_ok() {
        colored::control::set_override(false);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    match commands::dispatch(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Print to stderr via our consistent style. anyhow already
            // includes context chains in `{:#}`.
            output::err(&format!("{:#}", e));
            ExitCode::FAILURE
        }
    }
}
