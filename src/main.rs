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
    let cfg = config::load(&cli.config)?;
    // AIDEV-NOTE: aven preflight happens lazily via first aven call — its NotFound
    // error names the install prerequisite, a dedicated which(1) probe is extra code.
    let issues = jira::search(&cfg.jira)?;
    println!(
        "jira-aven-sync{} (config: {}, {} issues)",
        if cli.dry_run { " --dry-run" } else { "" },
        cli.config.display(),
        issues.len()
    );
    sync::run(&cfg, issues, cli.dry_run)?;
    Ok(())
}
