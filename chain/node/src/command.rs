use crate::{
    chain_spec,
    cli::{Cli, Subcommand},
    service,
};
use ialp_common_types::DomainId;
use sc_cli::SubstrateCli;
use sc_service::PartialComponents;

impl SubstrateCli for Cli {
    fn impl_name() -> String {
        "IALP Node".into()
    }

    fn impl_version() -> String {
        env!("SUBSTRATE_CLI_IMPL_VERSION").into()
    }

    fn description() -> String {
        env!("CARGO_PKG_DESCRIPTION").into()
    }

    fn author() -> String {
        env!("CARGO_PKG_AUTHORS").into()
    }

    fn support_url() -> String {
        "https://github.com/borisstoyanov/Interplanetary-Asynchronous-Ledger-Protocol".into()
    }

    fn copyright_start_year() -> i32 {
        2026
    }

    fn load_spec(&self, id: &str) -> Result<Box<dyn sc_service::ChainSpec>, String> {
        if let Some(domain) = self.domain {
            if id.is_empty() || id == "dev" || id == "local" {
                return Ok(Box::new(chain_spec::load_domain_chain_spec(
                    domain,
                    self.config.as_deref(),
                )?));
            }
        }

        if let Ok(domain) = id.parse::<DomainId>() {
            return Ok(Box::new(chain_spec::load_domain_chain_spec(domain, None)?));
        }

        Ok(Box::new(chain_spec::ChainSpec::from_json_file(
            std::path::PathBuf::from(id),
        )?))
    }
}

pub fn run() -> sc_cli::Result<()> {
    let cli = Cli::from_args();

    match &cli.subcommand {
        Some(Subcommand::Key(cmd)) => cmd.run(&cli),
        Some(Subcommand::BuildSpec(cmd)) => {
            require_domain(&cli, "build-spec")?;
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run(config.chain_spec, config.network))
        }
        Some(Subcommand::CheckBlock(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    import_queue,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, import_queue), task_manager))
            })
        }
        Some(Subcommand::ExportBlocks(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, config.database), task_manager))
            })
        }
        Some(Subcommand::ExportState(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, config.chain_spec), task_manager))
            })
        }
        Some(Subcommand::ImportBlocks(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    import_queue,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, import_queue), task_manager))
            })
        }
        Some(Subcommand::PurgeChain(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run(config.database))
        }
        Some(Subcommand::Revert(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    backend,
                    ..
                } = service::new_partial(&config)?;
                let aux_revert = Box::new(|client, _, blocks| {
                    sc_consensus_grandpa::revert(client, blocks)?;
                    Ok(())
                });
                Ok((cmd.run(client, backend, Some(aux_revert)), task_manager))
            })
        }
        Some(Subcommand::ChainInfo(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run::<ialp_runtime::opaque::Block>(&config))
        }
        None => {
            let domain = require_domain(&cli, "run")?;
            let loaded = chain_spec::domain_config_for_cli(domain, cli.config.as_deref())?;

            let runner = cli.create_runner(&cli.run)?;
            runner.run_node_until_exit(|config| async move {
                print_startup_context(&loaded, &config);

                match config.network.network_backend {
                    Some(sc_network::config::NetworkBackendType::Litep2p) => {
                        service::new_full::<sc_network::Litep2pNetworkBackend>(config)
                            .map_err(sc_cli::Error::Service)
                    }
                    Some(sc_network::config::NetworkBackendType::Libp2p) | None => {
                        service::new_full::<
                            sc_network::NetworkWorker<
                                ialp_runtime::opaque::Block,
                                <ialp_runtime::opaque::Block as sp_runtime::traits::Block>::Hash,
                            >,
                        >(config)
                        .map_err(sc_cli::Error::Service)
                    }
                }
            })
        }
    }
}

fn require_domain(cli: &Cli, context: &str) -> sc_cli::Result<DomainId> {
    cli.domain.ok_or_else(|| {
        sc_cli::Error::Input(format!(
            "`--domain <earth|moon|mars>` is required for {}",
            context
        ))
    })
}

fn print_startup_context(
    loaded: &ialp_common_config::LoadedDomainConfig,
    config: &sc_service::Configuration,
) {
    let role = if config.role.is_authority() {
        "authority"
    } else {
        "full"
    };

    println!(
        "IALP startup: domain={} chain_id={} epoch_length_seconds={} role={} config_source={}",
        loaded.config.domain_id,
        loaded.config.chain_id,
        loaded.config.epoch.length_seconds,
        role,
        loaded.source.display()
    );
}
