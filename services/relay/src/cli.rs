use clap::{Args, Parser, Subcommand};
use ialp_common_types::DomainId;

#[derive(Debug, Parser)]
#[command(name = "ialp-summary-relay")]
#[command(
    about = "Phase 3A relay for durable, blackout-aware delivery of certified IALP summary packages."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the relay HTTP server and background scheduler.
    Run(RunArgs),
    /// Inspect queued, scheduled, delivered, and acked packages.
    Status(StatusArgs),
    /// Show one persisted relay queue entry.
    Show(ShowArgs),
}

#[derive(Debug, Clone, Args)]
pub struct RunArgs {
    #[arg(long)]
    pub transport_config: Option<std::path::PathBuf>,
    #[arg(long, default_value_t = false)]
    pub json_logs: bool,
}

#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    #[arg(long)]
    pub transport_config: Option<std::path::PathBuf>,
    #[arg(long)]
    pub source_domain: Option<DomainId>,
    #[arg(long)]
    pub target_domain: Option<DomainId>,
    #[arg(long)]
    pub state: Option<String>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    #[arg(long)]
    pub transport_config: Option<std::path::PathBuf>,
    #[arg(long)]
    pub source_domain: DomainId,
    #[arg(long)]
    pub target_domain: DomainId,
    #[arg(long)]
    pub epoch: u64,
    #[arg(long)]
    pub package_hash: String,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}
