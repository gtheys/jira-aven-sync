mod adf;
mod aven;
mod config;
mod jira;
mod sync;

use clap::Parser;

/// Sync Jira issues into aven tasks (one-way, manual run).
#[derive(Parser)]
struct Cli {
    /// Print intended actions without mutating aven
    #[arg(long)]
    dry_run: bool,

    /// Path to TOML config file
    #[arg(long, value_name = "PATH", default_value = "config.toml")]
    config: std::path::PathBuf,
}

fn main() -> anyhow::Result<()> {
    // AIDEV-NOTE: blocking reqwest, no tokio — sequential sync of dozens of issues, latency irrelevant (locked in plan)
    let cli = Cli::parse();
    let _cfg = config::load(&cli.config)?;
    println!(
        "jira-aven-sync{} (config: {})",
        if cli.dry_run { " --dry-run" } else { "" },
        cli.config.display()
    );
    Ok(())
}
