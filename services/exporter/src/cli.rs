use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use ialp_common_types::{DomainId, EpochId};

#[derive(Debug, Parser)]
#[command(name = "ialp-summary-exporter")]
#[command(
    about = "Phase 2B exporter for GRANDPA-certified, storage-proven IALP summary packages with export inclusion proofs."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Watch finalized chain state and persist certified summary packages.
    Run(RunArgs),
    /// Inspect staged and export-certified summary status.
    Status(StatusArgs),
    /// Show one persisted certified package and its manifest.
    Show(ShowArgs),
}

#[derive(Debug, Clone, Args)]
pub struct RunArgs {
    #[arg(long)]
    pub domain: DomainId,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub node_url: Option<String>,
    #[arg(long)]
    pub store_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    #[arg(long)]
    pub domain: DomainId,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub node_url: Option<String>,
    #[arg(long)]
    pub store_dir: Option<PathBuf>,
    #[arg(long)]
    pub epoch: Option<EpochId>,
    #[arg(long)]
    pub target_domain: Option<DomainId>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    #[arg(long)]
    pub domain: DomainId,
    #[arg(long)]
    pub store_dir: PathBuf,
    #[arg(long)]
    pub epoch: EpochId,
    #[arg(long)]
    pub target_domain: DomainId,
    #[arg(long)]
    pub json: bool,
}
