use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use ialp_common_types::{DomainId, ExportId};

#[derive(Debug, Parser)]
#[command(name = "ialp-summary-importer")]
#[command(about = "Phase 2B importer for verifying export-proof-bearing IALP summary packages.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the importer HTTP server and background verification loop.
    Run(RunArgs),
    /// Verify a proof-bearing package and submit minimal observed-import records.
    Verify(VerifyArgs),
    /// Inspect importer-local verification and duplicate status.
    Status(StatusArgs),
    /// Show one importer-local record.
    Show(ShowArgs),
}

#[derive(Debug, Clone, Args)]
pub struct RunArgs {
    #[arg(long)]
    pub domain: DomainId,
    #[arg(long)]
    pub node_url: Option<String>,
    #[arg(long)]
    pub submitter_suri: String,
    #[arg(long)]
    pub transport_config: Option<PathBuf>,
    #[arg(long)]
    pub store_dir: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub json_logs: bool,
}

#[derive(Debug, Clone, Args)]
pub struct VerifyArgs {
    #[arg(long)]
    pub domain: DomainId,
    #[arg(long)]
    pub package: PathBuf,
    #[arg(long)]
    pub node_url: Option<String>,
    #[arg(long)]
    pub submitter_suri: String,
    #[arg(long)]
    pub transport_config: Option<PathBuf>,
    #[arg(long)]
    pub store_dir: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    #[arg(long)]
    pub domain: DomainId,
    #[arg(long)]
    pub store_dir: Option<PathBuf>,
    #[arg(long)]
    pub export_id: Option<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    #[arg(long)]
    pub domain: DomainId,
    #[arg(long)]
    pub export_id: String,
    #[arg(long)]
    pub store_dir: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

pub fn parse_export_id_hex(value: &str) -> anyhow::Result<ExportId> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(trimmed)?;
    let export_id: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected a 32-byte export id hex value"))?;
    Ok(export_id)
}
