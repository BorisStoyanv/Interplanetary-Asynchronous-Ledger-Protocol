use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "ialp-scenario-harness")]
#[command(about = "Phase 3B end-to-end scenario harness for IALP multi-domain flows.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run one reproducible end-to-end scenario.
    Run(RunArgs),
    /// Run the full canonical Phase 3B scenario suite sequentially.
    RunAll(RunAllArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, ValueEnum)]
pub enum ScenarioArg {
    EarthToMoonSuccess,
    EarthToMarsDelay,
    EarthToMarsBlackout,
    EarthToMoonRelayRestart,
}

#[derive(Debug, Clone, Args)]
pub struct RunArgs {
    #[arg(long)]
    pub scenario: ScenarioArg,
    #[arg(long)]
    pub artifacts_dir: Option<PathBuf>,
    #[arg(long)]
    pub ialp_node_bin: Option<PathBuf>,
    #[arg(long)]
    pub exporter_bin: Option<PathBuf>,
    #[arg(long)]
    pub relay_bin: Option<PathBuf>,
    #[arg(long)]
    pub importer_bin: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct RunAllArgs {
    #[arg(long)]
    pub artifacts_dir: Option<PathBuf>,
    #[arg(long)]
    pub ialp_node_bin: Option<PathBuf>,
    #[arg(long)]
    pub exporter_bin: Option<PathBuf>,
    #[arg(long)]
    pub relay_bin: Option<PathBuf>,
    #[arg(long)]
    pub importer_bin: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}
