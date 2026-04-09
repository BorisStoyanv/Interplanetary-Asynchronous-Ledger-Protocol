#[derive(Debug, clap::Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub subcommand: Option<Subcommand>,

    /// Domain identity is explicit in IALP. Operators should never start a node
    /// without knowing which planetary/local domain it belongs to.
    #[arg(long, global = true)]
    pub domain: Option<ialp_common_types::DomainId>,

    /// Optional domain config path. Defaults to `config/domains/<domain>.toml`.
    #[arg(long, global = true)]
    pub config: Option<std::path::PathBuf>,

    #[clap(flatten)]
    pub run: sc_cli::RunCmd,
}

#[derive(Debug, clap::Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Subcommand {
    /// Key management cli utilities.
    #[command(subcommand)]
    Key(sc_cli::KeySubcommand),

    /// Build a chain specification.
    BuildSpec(sc_cli::BuildSpecCmd),

    /// Validate blocks.
    CheckBlock(sc_cli::CheckBlockCmd),

    /// Export blocks.
    ExportBlocks(sc_cli::ExportBlocksCmd),

    /// Export the state of a given block into a chain spec.
    ExportState(sc_cli::ExportStateCmd),

    /// Import blocks.
    ImportBlocks(sc_cli::ImportBlocksCmd),

    /// Remove the whole chain.
    PurgeChain(sc_cli::PurgeChainCmd),

    /// Revert the chain to a previous state.
    Revert(sc_cli::RevertCmd),

    /// Db meta columns information.
    ChainInfo(sc_cli::ChainInfoCmd),
}
