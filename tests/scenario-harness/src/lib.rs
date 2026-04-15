mod cli;

use std::{
    collections::BTreeMap,
    fs::{self, File},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use clap::Parser;
use codec::{Decode, Encode};
use frame_support::traits::tokens::IdAmount;
use ialp_common_config::{
    load_domain_config, load_transport_config, BlackoutWindowConfig, DomainChainType,
    LoadedDomainConfig, LoadedTransportConfig, TransportConfig,
};
use ialp_common_types::{
    epoch_export_ids_storage_key, export_record_storage_key, observed_import_storage_key,
    storage_map_key, storage_value_key, summary_header_storage_key, AccountIdBytes, DomainId,
    EpochSummaryHeader, ExportId, ExportRecord, ExportStatus, ImportObservationStatus,
    ObservedImportRecord,
};
use jsonrpsee::{
    core::{client::ClientT, rpc_params},
    ws_client::{WsClient, WsClientBuilder},
};
use multiaddr::Multiaddr;
use pallet_ialp_transfers::Call as TransfersCall;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sp_core::{crypto::Ss58Codec, sr25519, Pair, H256};
use sp_runtime::{
    generic::Era,
    traits::{IdentifyAccount, Verify},
    MultiAddress, MultiSignature,
};
use tokio::{
    net::TcpStream,
    process::{Child, Command},
    time::sleep,
};

use crate::cli::{Cli, Commands, RunAllArgs, RunArgs, ScenarioArg};

const SUMMARY_SCHEMA_VERSION: u16 = 1;
const SERVICE_READINESS_TIMEOUT_SECS: u64 = 20;
const POLL_INTERVAL_MILLIS: u64 = 1_000;

pub async fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run(args) => {
            let summary = execute_run(args).await?;
            print_summary(&summary, true);
            Ok(())
        }
        Commands::RunAll(args) => execute_run_all(args).await,
    }
}

async fn execute_run(args: RunArgs) -> anyhow::Result<ScenarioSummary> {
    let kind = ScenarioKind::from(args.scenario);
    let binary_paths = BinaryPaths::from_args(
        args.ialp_node_bin,
        args.exporter_bin,
        args.relay_bin,
        args.importer_bin,
    )?;
    let artifact_root = artifact_root(args.artifacts_dir.as_deref(), kind.name())?;
    let mut runner = ScenarioRunner::new(kind, binary_paths, artifact_root)?;
    let result = runner.run().await;
    let summary = runner.finalize(result.as_ref().err()).await?;
    if let Err(error) = result {
        bail!("{error:#}");
    }
    Ok(summary)
}

async fn execute_run_all(args: RunAllArgs) -> anyhow::Result<()> {
    let binary_paths = BinaryPaths::from_args(
        args.ialp_node_bin,
        args.exporter_bin,
        args.relay_bin,
        args.importer_bin,
    )?;
    let root = artifact_root(args.artifacts_dir.as_deref(), "run-all")?;
    let scenarios = [
        ScenarioKind::EarthToMoonSuccess,
        ScenarioKind::EarthToMarsDelay,
        ScenarioKind::EarthToMarsBlackout,
        ScenarioKind::EarthToMoonRelayRestart,
    ];

    let mut summaries = Vec::with_capacity(scenarios.len());
    let mut failures = Vec::new();
    for scenario in scenarios {
        let scenario_root = root.join(scenario.name());
        let mut runner = ScenarioRunner::new(scenario, binary_paths.clone(), scenario_root)?;
        let result = runner.run().await;
        let summary = runner.finalize(result.as_ref().err()).await?;
        if let Err(error) = result {
            failures.push(format!("{}: {error:#}", scenario.name()));
        }
        summaries.push(summary);
    }

    let aggregate = json!({
        "schema_version": SUMMARY_SCHEMA_VERSION,
        "root_artifacts_dir": root,
        "scenarios": summaries,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&aggregate).expect("aggregate json serializes")
    );

    if failures.is_empty() {
        Ok(())
    } else {
        bail!("one or more scenarios failed:\n{}", failures.join("\n"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ScenarioKind {
    EarthToMoonSuccess,
    EarthToMarsDelay,
    EarthToMarsBlackout,
    EarthToMoonRelayRestart,
}

impl ScenarioKind {
    fn name(self) -> &'static str {
        match self {
            Self::EarthToMoonSuccess => "earth-to-moon-success",
            Self::EarthToMarsDelay => "earth-to-mars-delay",
            Self::EarthToMarsBlackout => "earth-to-mars-blackout",
            Self::EarthToMoonRelayRestart => "earth-to-moon-relay-restart",
        }
    }

    fn source_domain(self) -> DomainId {
        DomainId::Earth
    }

    fn target_domain(self) -> DomainId {
        match self {
            Self::EarthToMoonSuccess | Self::EarthToMoonRelayRestart => DomainId::Moon,
            Self::EarthToMarsDelay | Self::EarthToMarsBlackout => DomainId::Mars,
        }
    }

    fn has_source_follower(self) -> bool {
        matches!(self, Self::EarthToMoonSuccess)
    }

    fn link_delay_seconds(self) -> u64 {
        match self {
            Self::EarthToMoonSuccess => 2,
            Self::EarthToMarsDelay => 20,
            Self::EarthToMarsBlackout => 2,
            Self::EarthToMoonRelayRestart => 15,
        }
    }

    fn blackout_window_seconds(self) -> Option<u64> {
        matches!(self, Self::EarthToMarsBlackout).then_some(30)
    }

    fn blackout_start_offset_seconds(self) -> Option<u64> {
        matches!(self, Self::EarthToMarsBlackout).then_some(35)
    }

    fn stage_timeouts(self) -> StageTimeouts {
        StageTimeouts {
            node_readiness: Duration::from_secs(60),
            chain_finality_ready: Duration::from_secs(45),
            follower_sync_ready: matches!(self, Self::EarthToMoonSuccess)
                .then_some(Duration::from_secs(60)),
            transfer_included: Duration::from_secs(30),
            export_record_visible: Duration::from_secs(20),
            epoch_closed_and_summary_staged: Duration::from_secs(90),
            exporter_certified_and_submitted: Duration::from_secs(90),
            relay_scheduled: Duration::from_secs(20),
            relay_delivery_and_importer_ack: Duration::from_secs(match self {
                Self::EarthToMoonSuccess => 60,
                Self::EarthToMoonRelayRestart => 120,
                Self::EarthToMarsDelay => 150,
                Self::EarthToMarsBlackout => 180,
            }),
            destination_remote_observed: Duration::from_secs(30),
            completion_exporter_certified_and_submitted: Duration::from_secs(90),
            completion_relay_scheduled: Duration::from_secs(20),
            completion_relay_delivery_and_importer_ack: Duration::from_secs(90),
            source_remote_finalized: Duration::from_secs(60),
        }
    }
}

impl From<ScenarioArg> for ScenarioKind {
    fn from(value: ScenarioArg) -> Self {
        match value {
            ScenarioArg::EarthToMoonSuccess => Self::EarthToMoonSuccess,
            ScenarioArg::EarthToMarsDelay => Self::EarthToMarsDelay,
            ScenarioArg::EarthToMarsBlackout => Self::EarthToMarsBlackout,
            ScenarioArg::EarthToMoonRelayRestart => Self::EarthToMoonRelayRestart,
        }
    }
}

#[derive(Clone, Debug)]
struct StageTimeouts {
    node_readiness: Duration,
    chain_finality_ready: Duration,
    follower_sync_ready: Option<Duration>,
    transfer_included: Duration,
    export_record_visible: Duration,
    epoch_closed_and_summary_staged: Duration,
    exporter_certified_and_submitted: Duration,
    relay_scheduled: Duration,
    relay_delivery_and_importer_ack: Duration,
    destination_remote_observed: Duration,
    completion_exporter_certified_and_submitted: Duration,
    completion_relay_scheduled: Duration,
    completion_relay_delivery_and_importer_ack: Duration,
    source_remote_finalized: Duration,
}

#[derive(Clone)]
struct BinaryPaths {
    ialp_node: PathBuf,
    exporter: PathBuf,
    relay: PathBuf,
    importer: PathBuf,
}

impl BinaryPaths {
    fn from_args(
        ialp_node: Option<PathBuf>,
        exporter: Option<PathBuf>,
        relay: Option<PathBuf>,
        importer: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let target_debug = workspace_root().join("target/debug");
        let paths = Self {
            ialp_node: ialp_node.unwrap_or_else(|| target_debug.join(binary_name("ialp-node"))),
            exporter: exporter
                .unwrap_or_else(|| target_debug.join(binary_name("ialp-summary-exporter"))),
            relay: relay.unwrap_or_else(|| target_debug.join(binary_name("ialp-summary-relay"))),
            importer: importer
                .unwrap_or_else(|| target_debug.join(binary_name("ialp-summary-importer"))),
        };
        for (name, path) in [
            ("ialp-node", &paths.ialp_node),
            ("ialp-summary-exporter", &paths.exporter),
            ("ialp-summary-relay", &paths.relay),
            ("ialp-summary-importer", &paths.importer),
        ] {
            if !path.exists() {
                bail!("required binary {} not found at {}", name, path.display());
            }
        }
        Ok(paths)
    }
}

#[derive(Clone, Debug, Serialize)]
struct ScenarioSummary {
    schema_version: u16,
    scenario: ScenarioKind,
    success: bool,
    started_at_unix_ms: u64,
    ended_at_unix_ms: Option<u64>,
    failed_stage: Option<String>,
    failure_message: Option<String>,
    source_domain: DomainId,
    target_domain: DomainId,
    source_epoch_id: Option<u64>,
    extrinsic_hash: Option<String>,
    export_ids: Vec<String>,
    summary_hash: Option<String>,
    package_hash: Option<String>,
    completion_package_hash: Option<String>,
    final_relay_state: Option<String>,
    final_importer_package_state: Option<ialp_common_types::ImporterPackageState>,
    destination_observation: Option<DestinationObservationEvidence>,
    stage_results: Vec<StageResult>,
    artifact_paths: ArtifactPaths,
}

#[derive(Clone, Debug, Serialize)]
struct ArtifactPaths {
    root: String,
    configs_dir: String,
    chains_dir: String,
    stores_dir: String,
    logs_dir: String,
    summary_json: String,
}

#[derive(Clone, Debug, Serialize)]
struct DestinationObservationEvidence {
    export_id: String,
    source_domain: DomainId,
    target_domain: DomainId,
    source_epoch_id: u64,
    amount: u128,
    recipient: String,
    summary_hash: String,
    package_hash: String,
    observed_at_local_block_height: u32,
    observer_account: String,
    finalized_at_local_block_height: Option<u32>,
    finalizer_account: Option<String>,
    recipient_free_balance: String,
    status: ImportObservationStatus,
}

#[derive(Clone, Debug, Serialize)]
struct StageResult {
    stage: String,
    success: bool,
    timed_out: bool,
    duration_ms: u64,
    timeout_seconds: Option<u64>,
    details: serde_json::Value,
}

struct ScenarioRunner {
    scenario: ScenarioKind,
    binaries: BinaryPaths,
    layout: ArtifactLayout,
    processes: ProcessManager,
    summary: ScenarioSummary,
    node_configs: BTreeMap<DomainId, DomainRunConfig>,
    source_follower: Option<DomainRunConfig>,
    transport_config_path: Option<PathBuf>,
    transport_config: Option<TransportConfig>,
}

impl ScenarioRunner {
    fn new(
        scenario: ScenarioKind,
        binaries: BinaryPaths,
        artifact_root: PathBuf,
    ) -> anyhow::Result<Self> {
        let layout = ArtifactLayout::new(artifact_root)?;
        let summary_path = layout.root.join("summary.json");
        Ok(Self {
            scenario,
            binaries,
            processes: ProcessManager::default(),
            summary: ScenarioSummary {
                schema_version: SUMMARY_SCHEMA_VERSION,
                scenario,
                success: false,
                started_at_unix_ms: unix_now_millis()?,
                ended_at_unix_ms: None,
                failed_stage: None,
                failure_message: None,
                source_domain: scenario.source_domain(),
                target_domain: scenario.target_domain(),
                source_epoch_id: None,
                extrinsic_hash: None,
                export_ids: Vec::new(),
                summary_hash: None,
                package_hash: None,
                completion_package_hash: None,
                final_relay_state: None,
                final_importer_package_state: None,
                destination_observation: None,
                stage_results: Vec::new(),
                artifact_paths: ArtifactPaths {
                    root: display_path(&layout.root),
                    configs_dir: display_path(&layout.configs_dir),
                    chains_dir: display_path(&layout.chains_dir),
                    stores_dir: display_path(&layout.stores_dir),
                    logs_dir: display_path(&layout.logs_dir),
                    summary_json: display_path(&summary_path),
                },
            },
            layout,
            node_configs: BTreeMap::new(),
            source_follower: None,
            transport_config_path: None,
            transport_config: None,
        })
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        self.ensure_workspace_scenario_binaries().await?;
        self.ensure_node_runtime_wasm().await?;
        self.prepare_domain_configs()?;
        self.start_authority_nodes().await?;
        self.wait_for_node_readiness().await?;
        self.wait_for_chain_finality().await?;

        if self.scenario.has_source_follower() {
            self.start_source_follower().await?;
            self.wait_for_follower_sync("follower_sync_ready").await?;
        }

        self.prepare_transport_config()?;
        self.start_services().await?;
        self.wait_for_services().await?;

        let destination_rpc =
            ChainRpcClient::connect(&self.node_configs[&self.scenario.target_domain()].rpc_url())
                .await?;
        let recipient_account = account_id_from_seed("//Eve")?;
        let destination_initial_balance = destination_rpc
            .free_balance(recipient_account.clone())
            .await?
            .unwrap_or_default();

        let transfer = self.originate_transfer().await?;
        self.summary.extrinsic_hash = Some(hex_h256(transfer.extrinsic_hash));
        self.summary.source_epoch_id = Some(transfer.source_epoch_id);
        self.summary.export_ids.push(hex_hash(transfer.export_id));

        if self.scenario.has_source_follower() {
            self.wait_for_source_follower_convergence("follower_sync_after_transfer")
                .await?;
        }

        let summary_header = self.wait_for_epoch_commitment(transfer.export_id).await?;
        self.summary.summary_hash = Some(hex_hash(summary_header.summary_hash));

        if self.scenario.has_source_follower() {
            self.wait_for_source_follower_convergence("follower_sync_after_summary")
                .await?;
        }

        let exporter_record = self
            .wait_for_exporter_package(transfer.source_epoch_id)
            .await?;
        self.summary.package_hash = exporter_record.package_hash.clone();

        let relay_record = self.wait_for_relay_scheduled().await?;
        if matches!(self.scenario, ScenarioKind::EarthToMoonRelayRestart) {
            self.perform_relay_restart(&relay_record).await?;
        }
        if matches!(self.scenario, ScenarioKind::EarthToMarsDelay) {
            self.assert_chains_progress_during_delay().await?;
        }
        let relay_final = self.wait_for_relay_delivery().await?;
        self.summary.final_relay_state = Some(relay_state_name(&relay_final.state).to_string());

        let importer_record = self.wait_for_importer_terminal().await?;
        self.summary.final_importer_package_state = Some(importer_record.state);

        let observed = self
            .wait_for_destination_observation(
                transfer,
                &summary_header,
                destination_initial_balance,
                &recipient_account,
            )
            .await?;
        self.summary.destination_observation = Some(observed);

        let completion_exporter_record = self
            .wait_for_completion_exporter_package(transfer.export_id)
            .await?;
        self.summary.completion_package_hash = completion_exporter_record.package_hash.clone();

        self.wait_for_completion_relay_scheduled().await?;
        let completion_relay_final = self.wait_for_completion_relay_delivery().await?;
        self.summary.final_relay_state =
            Some(relay_state_name(&completion_relay_final.state).to_string());

        let source_importer_record = self.wait_for_source_importer_terminal().await?;
        self.summary.final_importer_package_state = Some(source_importer_record.state);

        self.wait_for_source_settlement_resolution(transfer)
            .await?;
        self.summary.success = true;
        Ok(())
    }

    async fn ensure_workspace_scenario_binaries(&self) -> anyhow::Result<()> {
        if ![
            &self.binaries.ialp_node,
            &self.binaries.exporter,
            &self.binaries.relay,
            &self.binaries.importer,
        ]
        .into_iter()
        .all(|path| is_workspace_target_binary(path))
        {
            return Ok(());
        }

        rebuild_workspace_scenario_binaries(&self.binaries.ialp_node).await
    }

    async fn ensure_node_runtime_wasm(&self) -> anyhow::Result<()> {
        match probe_node_runtime_wasm(&self.binaries.ialp_node).await? {
            NodeWasmProbe::Ready => Ok(()),
            NodeWasmProbe::MissingDevelopmentWasm => {
                if !is_workspace_target_binary(&self.binaries.ialp_node) {
                    bail!(
                        "node binary at {} was built without runtime wasm and is not a workspace target binary the harness can rebuild automatically",
                        self.binaries.ialp_node.display()
                    );
                }

                rebuild_workspace_node_binary(&self.binaries.ialp_node).await?;
                match probe_node_runtime_wasm(&self.binaries.ialp_node).await? {
                    NodeWasmProbe::Ready => Ok(()),
                    NodeWasmProbe::MissingDevelopmentWasm => bail!(
                        "node binary at {} still reports missing development wasm after rebuild",
                        self.binaries.ialp_node.display()
                    ),
                }
            }
        }
    }

    async fn finalize(&mut self, error: Option<&anyhow::Error>) -> anyhow::Result<ScenarioSummary> {
        if let Some(error) = error {
            if self.summary.failed_stage.is_none() {
                self.summary.failed_stage = Some("unhandled_error".into());
            }
            self.summary.failure_message = Some(format!("{error:#}"));
        }
        self.summary.ended_at_unix_ms = Some(unix_now_millis()?);
        let summary_path = self.layout.root.join("summary.json");
        let bytes =
            serde_json::to_vec_pretty(&self.summary).context("failed to serialize summary.json")?;
        fs::write(&summary_path, bytes)
            .with_context(|| format!("failed to write {}", summary_path.display()))?;
        self.processes.shutdown_all().await;
        Ok(self.summary.clone())
    }

    fn prepare_domain_configs(&mut self) -> anyhow::Result<()> {
        let reservations = allocate_node_ports(self.scenario.has_source_follower())?;
        let source_authority = reservations
            .get("earth-authority")
            .context("missing earth-authority port allocation")?;
        let source_config = write_domain_config(
            self.scenario.source_domain(),
            source_authority,
            &self.layout,
        )?;
        self.node_configs
            .insert(self.scenario.source_domain(), source_config);

        let target_name = format!("{}-authority", self.scenario.target_domain().as_str());
        let target_ports = reservations
            .get(target_name.as_str())
            .with_context(|| format!("missing {target_name} port allocation"))?;
        let target_config =
            write_domain_config(self.scenario.target_domain(), target_ports, &self.layout)?;
        self.node_configs
            .insert(self.scenario.target_domain(), target_config);

        if self.scenario.has_source_follower() {
            let follower_ports = reservations
                .get("earth-follower")
                .context("missing earth-follower port allocation")?;
            let follower_dir = self.layout.chains_dir.join("earth-follower");
            fs::create_dir_all(&follower_dir)
                .with_context(|| format!("failed to create {}", follower_dir.display()))?;
            let follower = DomainRunConfig {
                domain: DomainId::Earth,
                config_path: self
                    .node_configs
                    .get(&DomainId::Earth)
                    .expect("earth config exists")
                    .config_path
                    .clone(),
                rpc_port: follower_ports.rpc_port,
                p2p_port: follower_ports.p2p_port,
                prometheus_port: follower_ports.prometheus_port,
                base_path: follower_dir,
            };
            self.source_follower = Some(follower);
        }

        Ok(())
    }

    async fn start_authority_nodes(&mut self) -> anyhow::Result<()> {
        let earth = self
            .node_configs
            .get(&DomainId::Earth)
            .context("earth config missing")?
            .clone();
        self.start_authority_process("earth-authority", &earth, "//Alice")
            .await?;

        let target = self
            .node_configs
            .get(&self.scenario.target_domain())
            .context("target config missing")?
            .clone();
        let seed = match self.scenario.target_domain() {
            DomainId::Moon => "//Bob",
            DomainId::Mars => "//Charlie",
            DomainId::Earth => "//Alice",
        };
        let process_name = format!("{}-authority", self.scenario.target_domain().as_str());
        self.start_authority_process(&process_name, &target, seed)
            .await?;
        Ok(())
    }

    async fn start_authority_process(
        &mut self,
        process_name: &str,
        config: &DomainRunConfig,
        dev_seed: &str,
    ) -> anyhow::Result<()> {
        let mut args = vec![
            "--domain".to_string(),
            config.domain.as_str().to_string(),
            "--config".to_string(),
            display_path(&config.config_path),
            "--base-path".to_string(),
            display_path(&config.base_path),
            "--name".to_string(),
            process_name.to_string(),
            "--validator".to_string(),
            "--force-authoring".to_string(),
            "--unsafe-force-node-key-generation".to_string(),
            "--rpc-port".to_string(),
            config.rpc_port.to_string(),
            "--port".to_string(),
            config.p2p_port.to_string(),
            "--prometheus-port".to_string(),
            config.prometheus_port.to_string(),
        ];
        args.push(seed_flag(dev_seed).to_string());
        self.processes
            .spawn(
                process_name,
                &self.binaries.ialp_node,
                &args,
                &self.layout.logs_dir.join(format!("{process_name}.log")),
            )
            .await
    }

    async fn wait_for_node_readiness(&mut self) -> anyhow::Result<()> {
        let timeout = self.scenario.stage_timeouts().node_readiness;
        let earth_addr = self.node_configs[&DomainId::Earth].rpc_socket_addr();
        let target_addr = self.node_configs[&self.scenario.target_domain()].rpc_socket_addr();
        self.wait_for_stage("node_readiness", timeout, || async {
            let mut details = serde_json::Map::new();
            let earth_ready = tcp_ready(&earth_addr).await;
            details.insert("earth_authority_rpc_ready".into(), json!(earth_ready));
            let target_ready = tcp_ready(&target_addr).await;
            details.insert("target_authority_rpc_ready".into(), json!(target_ready));

            let ready = earth_ready && target_ready;
            if ready {
                StagePoll::ready((), serde_json::Value::Object(details))
            } else {
                StagePoll::pending(serde_json::Value::Object(details))
            }
        })
        .await
    }

    async fn wait_for_chain_finality(&mut self) -> anyhow::Result<()> {
        let timeout = self.scenario.stage_timeouts().chain_finality_ready;
        let earth_url = self.node_configs[&DomainId::Earth].rpc_url();
        let target_domain = self.scenario.target_domain();
        let target_url = self.node_configs[&target_domain].rpc_url();
        self.wait_for_stage("chain_finality_ready", timeout, || async {
            let earth_rpc = ChainRpcClient::connect(&earth_url).await?;
            let earth_finalized = earth_rpc.finalized_number().await?;
            let target_rpc = ChainRpcClient::connect(&target_url).await?;
            let target_finalized = target_rpc.finalized_number().await?;
            let details = json!({
                "earth_finalized": earth_finalized,
                "target_domain": target_domain,
                "target_finalized": target_finalized,
            });
            if earth_finalized >= 2 && target_finalized >= 2 {
                StagePoll::ready((), details)
            } else {
                StagePoll::pending(details)
            }
        })
        .await
    }

    async fn start_source_follower(&mut self) -> anyhow::Result<()> {
        let authority_rpc =
            ChainRpcClient::connect(&self.node_configs[&DomainId::Earth].rpc_url()).await?;
        let peer_id = authority_rpc.local_peer_id().await?;
        let addresses = authority_rpc.local_listen_addresses().await?;
        let bootnode = canonical_bootnode(&peer_id, &addresses)?;
        let follower = self
            .source_follower
            .clone()
            .context("source follower config missing")?;
        let earth = self
            .node_configs
            .get(&DomainId::Earth)
            .expect("earth config exists");
        let args = vec![
            "--domain".to_string(),
            "earth".to_string(),
            "--config".to_string(),
            display_path(&earth.config_path),
            "--base-path".to_string(),
            display_path(&follower.base_path),
            "--name".to_string(),
            "earth-follower".to_string(),
            "--unsafe-force-node-key-generation".to_string(),
            "--rpc-port".to_string(),
            follower.rpc_port.to_string(),
            "--port".to_string(),
            follower.p2p_port.to_string(),
            "--prometheus-port".to_string(),
            follower.prometheus_port.to_string(),
            "--bootnodes".to_string(),
            bootnode,
        ];
        self.processes
            .spawn(
                "earth-follower",
                &self.binaries.ialp_node,
                &args,
                &self.layout.logs_dir.join("earth-follower.log"),
            )
            .await
    }

    async fn wait_for_follower_sync(&mut self, stage_name: &str) -> anyhow::Result<()> {
        let timeout = self
            .scenario
            .stage_timeouts()
            .follower_sync_ready
            .unwrap_or_else(|| Duration::from_secs(60));
        self.wait_for_source_follower_convergence_inner(stage_name, timeout)
            .await
    }

    async fn wait_for_source_follower_convergence(
        &mut self,
        stage_name: &str,
    ) -> anyhow::Result<()> {
        self.wait_for_source_follower_convergence_inner(stage_name, Duration::from_secs(60))
            .await
    }

    async fn wait_for_source_follower_convergence_inner(
        &mut self,
        stage_name: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let earth_url = self.node_configs[&DomainId::Earth].rpc_url();
        let follower_url = self
            .source_follower
            .clone()
            .context("source follower config missing")?
            .rpc_url();
        self.wait_for_stage(stage_name, timeout, || async {
            let authority_rpc = match ChainRpcClient::connect(&earth_url).await {
                Ok(client) => client,
                Err(error) => {
                    return StagePoll::pending(json!({
                        "authority_connect_error": error.to_string(),
                    }))
                }
            };
            let follower_rpc = match ChainRpcClient::connect(&follower_url).await {
                Ok(client) => client,
                Err(error) => {
                    return StagePoll::pending(json!({
                        "follower_connect_error": error.to_string(),
                    }))
                }
            };
            let authority = authority_rpc.finalized_view().await?;
            let follower = follower_rpc.finalized_view().await?;
            let details = json!({
                "authority": authority,
                "follower": follower,
            });
            if authority["hash"] == follower["hash"] && authority["number"] == follower["number"] {
                StagePoll::ready((), details)
            } else {
                StagePoll::pending(details)
            }
        })
        .await
    }

    fn prepare_transport_config(&mut self) -> anyhow::Result<()> {
        let transport = build_transport_config(self.scenario, &self.layout, Utc::now())?;
        let transport_dir = self.layout.configs_dir.join("transport");
        fs::create_dir_all(&transport_dir)
            .with_context(|| format!("failed to create {}", transport_dir.display()))?;
        let path = transport_dir.join("local.toml");
        let contents =
            toml::to_string_pretty(&transport).context("failed to serialize transport config")?;
        fs::write(&path, contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
        self.transport_config = Some(transport);
        self.transport_config_path = Some(path);
        Ok(())
    }

    async fn start_services(&mut self) -> anyhow::Result<()> {
        let transport_path = self
            .transport_config_path
            .clone()
            .context("transport config missing")?;

        let relay_args = vec![
            "run".to_string(),
            "--transport-config".to_string(),
            display_path(&transport_path),
        ];
        self.processes
            .spawn(
                "relay",
                &self.binaries.relay,
                &relay_args,
                &self.layout.logs_dir.join("relay.log"),
            )
            .await?;

        for domain in [self.scenario.source_domain(), self.scenario.target_domain()] {
            let importer_store = self.layout.stores_dir.join("importer").join(domain.as_str());
            let importer_suri =
                load_domain_config(domain, Some(&self.node_configs[&domain].config_path))?
                    .config
                    .bootstrap
                    .importer_account_seed;
            let importer_args = vec![
                "run".to_string(),
                "--domain".to_string(),
                domain.as_str().to_string(),
                "--node-url".to_string(),
                self.node_configs[&domain].rpc_url(),
                "--submitter-suri".to_string(),
                importer_suri,
                "--transport-config".to_string(),
                display_path(&transport_path),
                "--store-dir".to_string(),
                display_path(&importer_store),
            ];
            self.processes
                .spawn(
                    &format!("{}-importer", domain.as_str()),
                    &self.binaries.importer,
                    &importer_args,
                    &self.layout
                        .logs_dir
                        .join(format!("{}-importer.log", domain.as_str())),
                )
                .await?;
        }

        for domain in [self.scenario.source_domain(), self.scenario.target_domain()] {
            let exporter_store = self.layout.stores_dir.join("exporter").join(domain.as_str());
            let exporter_args = vec![
                "run".to_string(),
                "--domain".to_string(),
                domain.as_str().to_string(),
                "--config".to_string(),
                display_path(&self.node_configs[&domain].config_path),
                "--node-url".to_string(),
                self.node_configs[&domain].rpc_url(),
                "--relay-url".to_string(),
                format!(
                    "http://{}",
                    self.transport_config
                        .as_ref()
                        .expect("transport exists")
                        .relay
                        .listen_addr
                ),
                "--transport-config".to_string(),
                display_path(&transport_path),
                "--store-dir".to_string(),
                display_path(&exporter_store),
            ];
            self.processes
                .spawn(
                    &format!("{}-exporter", domain.as_str()),
                    &self.binaries.exporter,
                    &exporter_args,
                    &self.layout
                        .logs_dir
                        .join(format!("{}-exporter.log", domain.as_str())),
                )
                .await?;
        }

        Ok(())
    }

    async fn wait_for_services(&mut self) -> anyhow::Result<()> {
        let transport = self.transport_config.clone().expect("transport exists");
        let relay_socket = format!(
            "127.0.0.1:{}",
            port_from_listen_addr(&transport.relay.listen_addr)?
        );
        let source_importer_socket = format!(
            "127.0.0.1:{}",
            port_from_listen_addr(
                &transport
                    .importers
                    .get(&self.scenario.source_domain())
                    .context("missing source importer listen addr")?
                    .listen_addr,
            )?
        );
        let target_importer_socket = format!(
            "127.0.0.1:{}",
            port_from_listen_addr(
                &transport
                    .importers
                    .get(&self.scenario.target_domain())
                    .context("missing target importer listen addr")?
                    .listen_addr,
            )?
        );
        self.wait_for_stage(
            "service_readiness",
            Duration::from_secs(SERVICE_READINESS_TIMEOUT_SECS),
            || async {
                let relay_ready = tcp_ready(&relay_socket).await;
                let source_importer_ready = tcp_ready(&source_importer_socket).await;
                let target_importer_ready = tcp_ready(&target_importer_socket).await;
                let details = json!({
                    "relay_ready": relay_ready,
                    "source_importer_ready": source_importer_ready,
                    "target_importer_ready": target_importer_ready,
                });
                if relay_ready && source_importer_ready && target_importer_ready {
                    StagePoll::ready((), details)
                } else {
                    StagePoll::pending(details)
                }
            },
        )
        .await
    }

    async fn originate_transfer(&mut self) -> anyhow::Result<TransferOutcome> {
        let timeout = self.scenario.stage_timeouts().transfer_included;
        let earth_rpc =
            ChainRpcClient::connect(&self.node_configs[&DomainId::Earth].rpc_url()).await?;
        let sender_pair = sr25519::Pair::from_string("//Alice", None)
            .map_err(|error| anyhow!("failed to load //Alice pair: {error}"))?;
        let sender_account = submitter_account_id(&sender_pair);
        let recipient = account_id_bytes_from_seed("//Eve")?;
        let runtime = earth_rpc.runtime_metadata().await?;
        let nonce = earth_rpc.account_next_index(&sender_account).await?;
        let target_domain = self.scenario.target_domain();
        let transfer_amount = 1_000_000_000_000u128;
        let extrinsic = build_transfer_extrinsic(
            &sender_pair,
            sender_account.clone(),
            runtime.spec_version,
            runtime.transaction_version,
            runtime.genesis_hash,
            nonce,
            target_domain,
            recipient,
            transfer_amount,
        )?;
        let extrinsic_hash = earth_rpc.submit_extrinsic(&extrinsic).await?;

        let inclusion = self
            .wait_for_stage("transfer_included", timeout, || async {
                let included = earth_rpc
                    .find_finalized_extrinsic(extrinsic_hash, &extrinsic)
                    .await?;
                let details = json!({
                    "extrinsic_hash": hex_h256(extrinsic_hash),
                    "included": included.as_ref().map(|value| json!({
                        "block_number": value.block_number,
                        "block_hash": hex_h256(value.block_hash),
                    })),
                });
                match included {
                    Some(value) => StagePoll::ready(value, details),
                    None => StagePoll::pending(details),
                }
            })
            .await?;

        let export = self
            .wait_for_stage(
                "export_record_visible",
                self.scenario.stage_timeouts().export_record_visible,
                || async {
                    let current_epoch = earth_rpc.current_epoch().await?.unwrap_or(0);
                    let mut matched = None;
                    let mut scanned_epochs = Vec::new();
                    for epoch_id in 0..=current_epoch {
                        scanned_epochs.push(epoch_id);
                        let export_ids = earth_rpc
                            .epoch_export_ids(epoch_id)
                            .await?
                            .unwrap_or_default();
                        for export_id in export_ids {
                            if let Some(record) = earth_rpc.export_record(export_id).await? {
                                if record.leaf.source_block_height == inclusion.block_number
                                    && record.leaf.target_domain == target_domain
                                    && record.leaf.amount == transfer_amount
                                    && record.leaf.recipient == recipient
                                {
                                    matched = Some((export_id, record));
                                    break;
                                }
                            }
                        }
                        if matched.is_some() {
                            break;
                        }
                    }
                    let details = json!({
                        "current_epoch": current_epoch,
                        "scanned_epochs": scanned_epochs,
                        "matched": matched.as_ref().map(|(export_id, record)| json!({
                            "export_id": hex_hash(*export_id),
                            "status": record.status,
                            "source_epoch_id": record.leaf.source_epoch_id,
                        })),
                    });
                    match matched {
                        Some((export_id, record)) => StagePoll::ready(
                            TransferOutcome {
                                extrinsic_hash,
                                export_id,
                                source_epoch_id: record.leaf.source_epoch_id,
                                recipient,
                                amount: record.leaf.amount,
                            },
                            details,
                        ),
                        None => StagePoll::pending(details),
                    }
                },
            )
            .await?;

        self.assert_sender_hold(&earth_rpc, &sender_account, export.amount)
            .await?;
        Ok(export)
    }

    async fn assert_sender_hold(
        &mut self,
        rpc: &ChainRpcClient,
        account: &ialp_runtime::AccountId,
        amount: u128,
    ) -> anyhow::Result<()> {
        let holds = rpc
            .balance_holds(account.clone())
            .await?
            .unwrap_or_default();
        let expected_id = ialp_runtime::RuntimeHoldReason::Transfers(
            pallet_ialp_transfers::HoldReason::CrossDomainTransfer,
        );
        let found = holds
            .iter()
            .any(|entry| entry.id == expected_id && entry.amount == amount);
        let details = json!({
            "holds": holds.iter().map(|entry| json!({
                "amount": entry.amount,
                "is_cross_domain_transfer": entry.id == expected_id,
            })).collect::<Vec<_>>(),
        });
        if found {
            self.record_stage_success("source_hold_exists", details)?;
            Ok(())
        } else {
            self.record_stage_failure("source_hold_exists", false, 0, None, details)?;
            bail!("expected source hold for cross-domain transfer was not found")
        }
    }

    async fn wait_for_epoch_commitment(
        &mut self,
        export_id: ExportId,
    ) -> anyhow::Result<EpochSummaryHeader> {
        let earth_rpc =
            ChainRpcClient::connect(&self.node_configs[&DomainId::Earth].rpc_url()).await?;
        let source_epoch_id = self.summary.source_epoch_id.expect("source epoch set");
        self.wait_for_stage(
            "epoch_closed_and_summary_staged",
            self.scenario
                .stage_timeouts()
                .epoch_closed_and_summary_staged,
            || async {
                let export_record = earth_rpc.export_record(export_id).await?;
                let summary = earth_rpc.summary_header(source_epoch_id).await?;
                let details = json!({
                    "export_status": export_record.as_ref().map(|record| record.status),
                    "summary_present": summary.is_some(),
                    "summary_hash": summary.as_ref().map(|header| hex_hash(header.summary_hash)),
                });
                match (export_record, summary) {
                    (Some(record), Some(header)) if record.status == ExportStatus::Exported => {
                        StagePoll::ready(header, details)
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn wait_for_exporter_package(
        &mut self,
        source_epoch_id: u64,
    ) -> anyhow::Result<ialp_summary_exporter::store::PackageRecord> {
        let exporter_store = ialp_summary_exporter::store::Store::new(
            self.layout
                .stores_dir
                .join("exporter")
                .join(self.scenario.source_domain().as_str()),
            self.scenario.source_domain(),
        )?;
        let target_domain = self.scenario.target_domain();
        self.wait_for_stage(
            "exporter_certified_and_submitted",
            self.scenario
                .stage_timeouts()
                .exporter_certified_and_submitted,
            || async {
                let index = exporter_store.load_index()?;
                let record = index.record(source_epoch_id, target_domain).cloned();
                let details = json!({
                    "latest_staged_epoch": index.latest_staged_epoch,
                    "latest_certified_epoch": index.latest_certified_epoch,
                    "record": record.as_ref().map(|entry| entry.json_summary()),
                });
                match record {
                    Some(record)
                        if record.status == ialp_summary_exporter::store::PackageStatus::Certified
                            && record.relay_submission_state
                                == ialp_summary_exporter::store::RelaySubmissionState::Submitted =>
                    {
                        StagePoll::ready(record, details)
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn wait_for_completion_exporter_package(
        &mut self,
        export_id: ExportId,
    ) -> anyhow::Result<ialp_summary_exporter::store::PackageRecord> {
        let exporter_store = ialp_summary_exporter::store::Store::new(
            self.layout
                .stores_dir
                .join("exporter")
                .join(self.scenario.target_domain().as_str()),
            self.scenario.target_domain(),
        )?;
        let target_domain = self.scenario.source_domain();
        let export_id_hex = hex_hash(export_id);
        self.wait_for_stage(
            "completion_exporter_certified_and_submitted",
            self.scenario
                .stage_timeouts()
                .completion_exporter_certified_and_submitted,
            || async {
                let index = exporter_store.load_index()?;
                let record = index
                    .packages
                    .iter()
                    .find(|record| {
                        record.target_domain == target_domain
                            && record.status
                                == ialp_summary_exporter::store::PackageStatus::Certified
                            && record.relay_submission_state
                                == ialp_summary_exporter::store::RelaySubmissionState::Submitted
                            && record.export_ids.iter().any(|id| id == &export_id_hex)
                            && record
                                .proof_kinds
                                .iter()
                                .any(|kind| kind == "finalized_import_v1")
                    })
                    .cloned();
                let details = json!({
                    "index": index.json_summary(),
                    "packages": index.packages.iter().map(|record| record.json_summary()).collect::<Vec<_>>(),
                });
                match record {
                    Some(record) => StagePoll::ready(record, details),
                    None => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn wait_for_relay_scheduled(
        &mut self,
    ) -> anyhow::Result<ialp_summary_relay::store::RelayPackageRecord> {
        let relay_store =
            ialp_summary_relay::store::Store::new(self.layout.stores_dir.join("relay"))?;
        let source_domain = self.scenario.source_domain();
        let target_domain = self.scenario.target_domain();
        let package_hash = self.summary.package_hash.clone();
        self.wait_for_stage(
            "relay_scheduled",
            self.scenario.stage_timeouts().relay_scheduled,
            || async {
                let index = relay_store.load_index()?;
                let record = index
                    .packages
                    .iter()
                    .find(|record| {
                        record.source_domain == source_domain
                            && record.target_domain == target_domain
                            && package_hash
                                .as_ref()
                                .map(|hash| hash == &record.package_hash)
                                .unwrap_or(true)
                    })
                    .cloned();
                let details = json!({
                    "packages": index.packages.iter().map(|record| record.json_summary()).collect::<Vec<_>>(),
                });
                match record {
                    Some(record)
                        if matches!(
                            record.state,
                            ialp_summary_relay::store::RelayQueueState::Scheduled
                                | ialp_summary_relay::store::RelayQueueState::BlockedByBlackout
                                | ialp_summary_relay::store::RelayQueueState::Delivered
                                | ialp_summary_relay::store::RelayQueueState::ImporterAcked
                        ) =>
                    {
                        StagePoll::ready(record, details)
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn perform_relay_restart(
        &mut self,
        relay_record: &ialp_summary_relay::store::RelayPackageRecord,
    ) -> anyhow::Result<()> {
        let payload_exists = Path::new(&relay_record.payload_path).exists();
        let entry_exists = Path::new(&relay_record.entry_path).exists();
        let details = json!({
            "state": relay_state_name(&relay_record.state),
            "entry_path": relay_record.entry_path,
            "payload_path": relay_record.payload_path,
            "entry_exists": entry_exists,
            "payload_exists": payload_exists,
        });
        if !(payload_exists && entry_exists) {
            self.record_stage_failure("relay_restart_precondition", false, 0, None, details)?;
            bail!("relay persistence precondition failed before restart");
        }
        self.record_stage_success("relay_restart_precondition", details)?;

        self.processes.stop("relay").await;
        sleep(Duration::from_secs(2)).await;

        let transport_path = self
            .transport_config_path
            .clone()
            .context("transport config missing")?;
        let relay_args = vec![
            "run".to_string(),
            "--transport-config".to_string(),
            display_path(&transport_path),
        ];
        self.processes
            .spawn(
                "relay",
                &self.binaries.relay,
                &relay_args,
                &self.layout.logs_dir.join("relay-restarted.log"),
            )
            .await?;
        self.wait_for_services().await
    }

    async fn assert_chains_progress_during_delay(&mut self) -> anyhow::Result<()> {
        let earth_rpc =
            ChainRpcClient::connect(&self.node_configs[&DomainId::Earth].rpc_url()).await?;
        let target_rpc =
            ChainRpcClient::connect(&self.node_configs[&self.scenario.target_domain()].rpc_url())
                .await?;
        let earth_start = earth_rpc.finalized_number().await?;
        let target_start = target_rpc.finalized_number().await?;
        sleep(Duration::from_secs(14)).await;
        let earth_end = earth_rpc.finalized_number().await?;
        let target_end = target_rpc.finalized_number().await?;
        let relay_store =
            ialp_summary_relay::store::Store::new(self.layout.stores_dir.join("relay"))?;
        let index = relay_store.load_index()?;
        let relay_state = index
            .packages
            .iter()
            .find(|record| record.target_domain == self.scenario.target_domain())
            .map(|record| relay_state_name(&record.state).to_string());
        let details = json!({
            "earth_finalized_start": earth_start,
            "earth_finalized_end": earth_end,
            "target_domain": self.scenario.target_domain(),
            "target_finalized_start": target_start,
            "target_finalized_end": target_end,
            "relay_state": relay_state,
        });
        if earth_end >= earth_start.saturating_add(2)
            && target_end >= target_start.saturating_add(2)
        {
            self.record_stage_success("chains_progress_during_delay", details)?;
            Ok(())
        } else {
            self.record_stage_failure("chains_progress_during_delay", false, 0, None, details)?;
            bail!("source/destination chains did not continue finalizing during relay delay")
        }
    }

    async fn wait_for_relay_delivery(
        &mut self,
    ) -> anyhow::Result<ialp_summary_relay::store::RelayPackageRecord> {
        let relay_store =
            ialp_summary_relay::store::Store::new(self.layout.stores_dir.join("relay"))?;
        let require_blocked = matches!(self.scenario, ScenarioKind::EarthToMarsBlackout);
        let target_domain = self.scenario.target_domain();
        self.wait_for_stage(
            "relay_delivery_and_importer_ack",
            self.scenario.stage_timeouts().relay_delivery_and_importer_ack,
            || async {
                let index = relay_store.load_index()?;
                let mut final_record = None;
                for record in &index.packages {
                    if record.target_domain != target_domain {
                        continue;
                    }
                    final_record = Some(record.clone());
                }
                let saw_blocked = final_record
                    .as_ref()
                    .map(|record| record.ever_blocked_by_blackout)
                    .unwrap_or(false);
                let details = json!({
                    "require_blocked": require_blocked,
                    "saw_blocked": saw_blocked,
                    "packages": index.packages.iter().map(|record| record.json_summary()).collect::<Vec<_>>(),
                });
                match final_record {
                    Some(record)
                        if matches!(
                            record.state,
                            ialp_summary_relay::store::RelayQueueState::ImporterAcked
                        ) && (!require_blocked || saw_blocked) =>
                    {
                        StagePoll::ready(record, details)
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn wait_for_completion_relay_scheduled(
        &mut self,
    ) -> anyhow::Result<ialp_summary_relay::store::RelayPackageRecord> {
        let relay_store =
            ialp_summary_relay::store::Store::new(self.layout.stores_dir.join("relay"))?;
        let source_domain = self.scenario.target_domain();
        let target_domain = self.scenario.source_domain();
        let package_hash = self.summary.completion_package_hash.clone();
        self.wait_for_stage(
            "completion_relay_scheduled",
            self.scenario.stage_timeouts().completion_relay_scheduled,
            || async {
                let index = relay_store.load_index()?;
                let record = index
                    .packages
                    .iter()
                    .find(|record| {
                        record.source_domain == source_domain
                            && record.target_domain == target_domain
                            && package_hash
                                .as_ref()
                                .map(|hash| hash == &record.package_hash)
                                .unwrap_or(true)
                    })
                    .cloned();
                let details = json!({
                    "packages": index.packages.iter().map(|record| record.json_summary()).collect::<Vec<_>>(),
                });
                match record {
                    Some(record)
                        if matches!(
                            record.state,
                            ialp_summary_relay::store::RelayQueueState::Scheduled
                                | ialp_summary_relay::store::RelayQueueState::BlockedByBlackout
                                | ialp_summary_relay::store::RelayQueueState::Delivered
                                | ialp_summary_relay::store::RelayQueueState::ImporterAcked
                        ) =>
                    {
                        StagePoll::ready(record, details)
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn wait_for_completion_relay_delivery(
        &mut self,
    ) -> anyhow::Result<ialp_summary_relay::store::RelayPackageRecord> {
        let relay_store =
            ialp_summary_relay::store::Store::new(self.layout.stores_dir.join("relay"))?;
        let source_domain = self.scenario.target_domain();
        let target_domain = self.scenario.source_domain();
        self.wait_for_stage(
            "completion_relay_delivery_and_importer_ack",
            self.scenario
                .stage_timeouts()
                .completion_relay_delivery_and_importer_ack,
            || async {
                let index = relay_store.load_index()?;
                let record = index
                    .packages
                    .iter()
                    .find(|record| {
                        record.source_domain == source_domain
                            && record.target_domain == target_domain
                    })
                    .cloned();
                let details = json!({
                    "packages": index.packages.iter().map(|record| record.json_summary()).collect::<Vec<_>>(),
                });
                match record {
                    Some(record)
                        if matches!(
                            record.state,
                            ialp_summary_relay::store::RelayQueueState::ImporterAcked
                        ) =>
                    {
                        StagePoll::ready(record, details)
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn wait_for_importer_terminal(
        &mut self,
    ) -> anyhow::Result<ialp_summary_importer::store::PackageRecord> {
        let importer_store = ialp_summary_importer::store::Store::new(
            self.layout
                .stores_dir
                .join("importer")
                .join(self.scenario.target_domain().as_str()),
            self.scenario.target_domain(),
        )?;
        let target_domain = self.scenario.target_domain();
        self.wait_for_stage(
            "importer_terminal_state",
            self.scenario.stage_timeouts().relay_delivery_and_importer_ack,
            || async {
                let index = importer_store.load_index()?;
                let record = index
                    .packages
                    .iter()
                    .find(|record| record.target_domain == target_domain)
                    .cloned();
                let details = json!({
                    "packages": index.packages.iter().map(|record| record.json_summary()).collect::<Vec<_>>(),
                    "records": index.imports.iter().map(|record| record.json_summary()).collect::<Vec<_>>(),
                });
                match record {
                    Some(record)
                        if record.state == ialp_common_types::ImporterPackageState::AckedVerified =>
                    {
                        StagePoll::ready(record, details)
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn wait_for_source_importer_terminal(
        &mut self,
    ) -> anyhow::Result<ialp_summary_importer::store::PackageRecord> {
        let importer_store = ialp_summary_importer::store::Store::new(
            self.layout
                .stores_dir
                .join("importer")
                .join(self.scenario.source_domain().as_str()),
            self.scenario.source_domain(),
        )?;
        let package_hash = self.summary.completion_package_hash.clone();
        let completion_source_domain = self.scenario.target_domain();
        let completion_target_domain = self.scenario.source_domain();
        self.wait_for_stage(
            "source_importer_terminal_state",
            self.scenario
                .stage_timeouts()
                .completion_relay_delivery_and_importer_ack,
            || async {
                let index = importer_store.load_index()?;
                let record = index
                    .packages
                    .iter()
                    .find(|record| {
                        record.source_domain == completion_source_domain
                            && record.target_domain == completion_target_domain
                            && package_hash
                                .as_ref()
                                .map(|hash| hash == &record.package_hash)
                                .unwrap_or(true)
                    })
                    .cloned();
                let details = json!({
                    "packages": index.packages.iter().map(|record| record.json_summary()).collect::<Vec<_>>(),
                    "records": index.imports.iter().map(|record| record.json_summary()).collect::<Vec<_>>(),
                });
                match record {
                    Some(record)
                        if record.state == ialp_common_types::ImporterPackageState::AckedVerified =>
                    {
                        StagePoll::ready(record, details)
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn wait_for_destination_observation(
        &mut self,
        transfer: TransferOutcome,
        summary_header: &EpochSummaryHeader,
        destination_initial_balance: u128,
        recipient_account: &ialp_runtime::AccountId,
    ) -> anyhow::Result<DestinationObservationEvidence> {
        let target_rpc =
            ChainRpcClient::connect(&self.node_configs[&self.scenario.target_domain()].rpc_url())
                .await?;
        let source_domain = self.scenario.source_domain();
        let target_domain = self.scenario.target_domain();
        let expected_package_hash = self
            .summary
            .package_hash
            .clone()
            .context("package hash missing before destination observation")?;
        let expected_summary_hash = summary_header.summary_hash;
        self.wait_for_stage(
            "destination_remote_finalized",
            self.scenario.stage_timeouts().destination_remote_observed,
            || async {
                let record = target_rpc.observed_import(transfer.export_id).await?;
                let recipient_free_balance = target_rpc
                    .free_balance(recipient_account.clone())
                    .await?
                    .unwrap_or_default();
                let details = json!({
                    "record": record.as_ref().map(|record| json!({
                        "export_id": hex_hash(record.export_id),
                        "source_domain": record.source_domain,
                        "target_domain": record.target_domain,
                        "source_epoch_id": record.source_epoch_id,
                        "amount": record.amount,
                        "recipient": hex_hash(record.recipient),
                        "summary_hash": hex_hash(record.summary_hash),
                        "package_hash": hex_hash(record.package_hash),
                        "status": record.status,
                        "finalized_at_local_block_height": record.finalized_at_local_block_height,
                    })),
                    "recipient_free_balance": recipient_free_balance.to_string(),
                    "expected_recipient_free_balance": destination_initial_balance
                        .saturating_add(transfer.amount)
                        .to_string(),
                });
                match record {
                    Some(record)
                        if record.status == ImportObservationStatus::RemoteFinalized
                            && record.source_domain == source_domain
                            && record.target_domain == target_domain
                            && record.source_epoch_id == transfer.source_epoch_id
                            && record.amount == transfer.amount
                            && record.recipient == transfer.recipient
                            && record.summary_hash == expected_summary_hash
                            && expected_package_hash == hex_hash(record.package_hash)
                            && recipient_free_balance
                                == destination_initial_balance.saturating_add(transfer.amount) =>
                    {
                        StagePoll::ready(
                            DestinationObservationEvidence {
                                export_id: hex_hash(record.export_id),
                                source_domain: record.source_domain,
                                target_domain: record.target_domain,
                                source_epoch_id: record.source_epoch_id,
                                amount: record.amount,
                                recipient: hex_hash(record.recipient),
                                summary_hash: hex_hash(record.summary_hash),
                                package_hash: hex_hash(record.package_hash),
                                observed_at_local_block_height: record
                                    .observed_at_local_block_height,
                                observer_account: hex_hash(record.observer_account),
                                finalized_at_local_block_height: record
                                    .finalized_at_local_block_height,
                                finalizer_account: record
                                    .finalizer_account
                                    .map(hex_hash),
                                recipient_free_balance: recipient_free_balance.to_string(),
                                status: record.status,
                            },
                            details,
                        )
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await
    }

    async fn wait_for_source_settlement_resolution(
        &mut self,
        transfer: TransferOutcome,
    ) -> anyhow::Result<()> {
        let source_rpc =
            ChainRpcClient::connect(&self.node_configs[&self.scenario.source_domain()].rpc_url())
                .await?;
        let sender_account = account_id_from_seed("//Alice")?;
        let expected_completion_package_hash = self
            .summary
            .completion_package_hash
            .clone()
            .context("completion package hash missing before source settlement resolution")?;
        self.wait_for_stage(
            "source_remote_finalized",
            self.scenario.stage_timeouts().source_remote_finalized,
            || async {
                let export_record = source_rpc.export_record(transfer.export_id).await?;
                let holds = source_rpc
                    .balance_holds(sender_account.clone())
                    .await?
                    .unwrap_or_default();
                let expected_id = ialp_runtime::RuntimeHoldReason::Transfers(
                    pallet_ialp_transfers::HoldReason::CrossDomainTransfer,
                );
                let hold_present = holds
                    .iter()
                    .any(|entry| entry.id == expected_id && entry.amount == transfer.amount);
                let details = json!({
                    "export_record": export_record.as_ref().map(|record| json!({
                        "status": record.status,
                        "completion_summary_hash": record.completion_summary_hash.map(hex_hash),
                        "completion_package_hash": record.completion_package_hash.map(hex_hash),
                        "resolved_at_source_block_height": record.resolved_at_source_block_height,
                    })),
                    "hold_present": hold_present,
                    "holds": holds.iter().map(|entry| json!({
                        "amount": entry.amount,
                        "is_cross_domain_transfer": entry.id == expected_id,
                    })).collect::<Vec<_>>(),
                });
                match export_record {
                    Some(record)
                        if record.status == ExportStatus::RemoteFinalized
                            && record
                                .completion_package_hash
                                .map(hex_hash)
                                .as_deref()
                                == Some(expected_completion_package_hash.as_str())
                            && record.resolved_at_source_block_height.is_some()
                            && !hold_present =>
                    {
                        StagePoll::ready((), details)
                    }
                    _ => StagePoll::pending(details),
                }
            },
        )
        .await?;
        Ok(())
    }

    async fn wait_for_stage<T, F, Fut>(
        &mut self,
        stage_name: &str,
        timeout: Duration,
        mut poll: F,
    ) -> anyhow::Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<StagePoll<T>>>,
    {
        let start = Instant::now();
        let mut last_detail = json!({});
        loop {
            match poll().await {
                Ok(StagePoll::Ready { value, details }) => {
                    let duration_ms = elapsed_millis(start);
                    self.record_stage_success_with_duration(stage_name, duration_ms, details)?;
                    return Ok(value);
                }
                Ok(StagePoll::Pending { details }) => {
                    last_detail = details;
                }
                Err(error) => {
                    let duration_ms = elapsed_millis(start);
                    let details = json!({
                        "error": format!("{error:#}"),
                        "last_observed_state": last_detail,
                    });
                    self.record_stage_failure(
                        stage_name,
                        false,
                        duration_ms,
                        None,
                        details.clone(),
                    )?;
                    bail!("{stage_name} failed: {error:#}");
                }
            }

            if start.elapsed() >= timeout {
                let duration_ms = elapsed_millis(start);
                let timeout_seconds = timeout.as_secs();
                let details = json!({
                    "last_observed_state": last_detail,
                });
                self.record_stage_failure(
                    stage_name,
                    true,
                    duration_ms,
                    Some(timeout_seconds),
                    details,
                )?;
                bail!("{stage_name} timed out after {timeout_seconds}s");
            }
            sleep(Duration::from_millis(POLL_INTERVAL_MILLIS)).await;
        }
    }

    fn record_stage_success(
        &mut self,
        stage_name: &str,
        details: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.record_stage_success_with_duration(stage_name, 0, details)
    }

    fn record_stage_success_with_duration(
        &mut self,
        stage_name: &str,
        duration_ms: u64,
        details: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.summary.stage_results.push(StageResult {
            stage: stage_name.to_string(),
            success: true,
            timed_out: false,
            duration_ms,
            timeout_seconds: None,
            details,
        });
        Ok(())
    }

    fn record_stage_failure(
        &mut self,
        stage_name: &str,
        timed_out: bool,
        duration_ms: u64,
        timeout_seconds: Option<u64>,
        details: serde_json::Value,
    ) -> anyhow::Result<()> {
        if self.summary.failed_stage.is_none() {
            self.summary.failed_stage = Some(stage_name.to_string());
        }
        self.summary.stage_results.push(StageResult {
            stage: stage_name.to_string(),
            success: false,
            timed_out,
            duration_ms,
            timeout_seconds,
            details,
        });
        Ok(())
    }
}

enum StagePoll<T> {
    Ready {
        value: T,
        details: serde_json::Value,
    },
    Pending {
        details: serde_json::Value,
    },
}

impl<T> StagePoll<T> {
    fn ready(value: T, details: serde_json::Value) -> anyhow::Result<Self> {
        Ok(Self::Ready { value, details })
    }

    fn pending(details: serde_json::Value) -> anyhow::Result<Self> {
        Ok(Self::Pending { details })
    }
}

#[derive(Clone)]
struct DomainRunConfig {
    domain: DomainId,
    config_path: PathBuf,
    rpc_port: u16,
    p2p_port: u16,
    prometheus_port: u16,
    base_path: PathBuf,
}

impl DomainRunConfig {
    fn rpc_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.rpc_port)
    }

    fn rpc_socket_addr(&self) -> String {
        format!("127.0.0.1:{}", self.rpc_port)
    }
}

#[derive(Clone, Debug)]
struct NodePorts {
    rpc_port: u16,
    p2p_port: u16,
    prometheus_port: u16,
}

#[derive(Default)]
struct ProcessManager {
    children: BTreeMap<String, ManagedProcess>,
}

impl ProcessManager {
    async fn spawn(
        &mut self,
        name: &str,
        binary: &Path,
        args: &[String],
        log_path: &Path,
    ) -> anyhow::Result<()> {
        let log_file = File::create(log_path)
            .with_context(|| format!("failed to create {}", log_path.display()))?;
        let stderr = log_file
            .try_clone()
            .with_context(|| format!("failed to clone {}", log_path.display()))?;
        let child = Command::new(binary)
            .args(args)
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn {} from {}", name, binary.display()))?;
        self.children
            .insert(name.to_string(), ManagedProcess { child: Some(child) });
        Ok(())
    }

    async fn stop(&mut self, name: &str) {
        if let Some(process) = self.children.get_mut(name) {
            process.stop().await;
        }
    }

    async fn shutdown_all(&mut self) {
        let names = self.children.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.stop(&name).await;
        }
    }
}

struct ManagedProcess {
    child: Option<Child>,
}

impl ManagedProcess {
    async fn stop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        }
        self.child = None;
    }
}

struct ArtifactLayout {
    root: PathBuf,
    configs_dir: PathBuf,
    chains_dir: PathBuf,
    stores_dir: PathBuf,
    logs_dir: PathBuf,
}

impl ArtifactLayout {
    fn new(root: PathBuf) -> anyhow::Result<Self> {
        let configs_dir = root.join("configs");
        let chains_dir = root.join("chains");
        let stores_dir = root.join("stores");
        let logs_dir = root.join("logs");
        for dir in [&configs_dir, &chains_dir, &stores_dir, &logs_dir] {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        Ok(Self {
            root,
            configs_dir,
            chains_dir,
            stores_dir,
            logs_dir,
        })
    }
}

#[derive(Clone, Copy)]
struct TransferOutcome {
    extrinsic_hash: H256,
    export_id: ExportId,
    source_epoch_id: u64,
    recipient: AccountIdBytes,
    amount: u128,
}

struct IncludedExtrinsic {
    block_number: u32,
    block_hash: H256,
}

struct RuntimeMetadata {
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NodeWasmProbe {
    Ready,
    MissingDevelopmentWasm,
}

struct ChainRpcClient {
    client: WsClient,
}

impl ChainRpcClient {
    async fn connect(url: impl AsRef<str>) -> anyhow::Result<Self> {
        let client = WsClientBuilder::default()
            .build(url.as_ref())
            .await
            .with_context(|| format!("failed to connect websocket {}", url.as_ref()))?;
        Ok(Self { client })
    }

    async fn finalized_head_hash(&self) -> anyhow::Result<H256> {
        let hash: String = self
            .client
            .request("chain_getFinalizedHead", rpc_params![])
            .await
            .context("failed to fetch finalized head")?;
        parse_h256(&hash)
    }

    async fn finalized_number(&self) -> anyhow::Result<u32> {
        let hash = self.finalized_head_hash().await?;
        self.header_number(Some(hash)).await
    }

    async fn finalized_view(&self) -> anyhow::Result<serde_json::Value> {
        let hash = self.finalized_head_hash().await?;
        let number = self.header_number(Some(hash)).await?;
        Ok(json!({
            "number": number,
            "hash": hex_h256(hash),
        }))
    }

    async fn header_number(&self, hash: Option<H256>) -> anyhow::Result<u32> {
        let hash_hex = hash.map(hex_h256);
        let header: Option<RpcHeader> = self
            .client
            .request("chain_getHeader", rpc_params![hash_hex])
            .await
            .context("failed to fetch header")?;
        let header = header.context("header missing")?;
        parse_hex_u32(&header.number)
    }

    async fn block_hash(&self, number: u32) -> anyhow::Result<Option<H256>> {
        let hash: Option<String> = self
            .client
            .request("chain_getBlockHash", rpc_params![number])
            .await
            .context("failed to fetch block hash")?;
        hash.map(|value| parse_h256(&value)).transpose()
    }

    async fn block(&self, hash: H256) -> anyhow::Result<Option<RpcBlockResponse>> {
        let hash_hex = hex_h256(hash);
        self.client
            .request("chain_getBlock", rpc_params![hash_hex])
            .await
            .context("failed to fetch block")
    }

    async fn runtime_metadata(&self) -> anyhow::Result<RuntimeMetadata> {
        let version: RuntimeVersionView = self
            .client
            .request("state_getRuntimeVersion", rpc_params![])
            .await
            .context("failed to fetch runtime version")?;
        let genesis_hash = self
            .block_hash(0)
            .await?
            .context("missing genesis block hash")?;
        Ok(RuntimeMetadata {
            spec_version: version.spec_version,
            transaction_version: version.transaction_version,
            genesis_hash,
        })
    }

    async fn account_next_index(&self, account: &ialp_runtime::AccountId) -> anyhow::Result<u32> {
        let ss58 = account.to_ss58check();
        self.client
            .request("system_accountNextIndex", rpc_params![ss58])
            .await
            .context("failed to fetch account nonce")
    }

    async fn submit_extrinsic(&self, bytes: &[u8]) -> anyhow::Result<H256> {
        let encoded = format!("0x{}", hex::encode(bytes));
        let hash: String = self
            .client
            .request("author_submitExtrinsic", rpc_params![encoded])
            .await
            .context("failed to submit extrinsic")?;
        parse_h256(&hash)
    }

    async fn load_storage_value<T: Decode>(&self, key: Vec<u8>) -> anyhow::Result<Option<T>> {
        let hex_key = format!("0x{}", hex::encode(key));
        let response: Option<String> = self
            .client
            .request("state_getStorage", rpc_params![hex_key])
            .await
            .context("failed to query storage")?;
        response
            .map(|value| {
                let raw = decode_hex_bytes(&value)?;
                T::decode(&mut &raw[..]).map_err(|error| anyhow!("scale decode failed: {error}"))
            })
            .transpose()
    }

    async fn current_epoch(&self) -> anyhow::Result<Option<u64>> {
        self.load_storage_value(storage_value_key(b"Epochs", b"CurrentEpoch"))
            .await
    }

    async fn summary_header(&self, epoch_id: u64) -> anyhow::Result<Option<EpochSummaryHeader>> {
        self.load_storage_value(summary_header_storage_key(epoch_id))
            .await
    }

    async fn export_record(&self, export_id: ExportId) -> anyhow::Result<Option<ExportRecord>> {
        self.load_storage_value(export_record_storage_key(export_id))
            .await
    }

    async fn epoch_export_ids(&self, epoch_id: u64) -> anyhow::Result<Option<Vec<ExportId>>> {
        self.load_storage_value(epoch_export_ids_storage_key(epoch_id))
            .await
    }

    async fn observed_import(
        &self,
        export_id: ExportId,
    ) -> anyhow::Result<Option<ObservedImportRecord>> {
        self.load_storage_value(observed_import_storage_key(export_id))
            .await
    }

    async fn balance_holds(
        &self,
        account: ialp_runtime::AccountId,
    ) -> anyhow::Result<Option<Vec<IdAmount<ialp_runtime::RuntimeHoldReason, u128>>>> {
        self.load_storage_value(storage_map_key(b"Balances", b"Holds", &account.encode()))
            .await
    }

    async fn free_balance(&self, account: ialp_runtime::AccountId) -> anyhow::Result<Option<u128>> {
        type AccountInfo = frame_system::AccountInfo<u32, pallet_balances::AccountData<u128>>;
        let account_info: Option<AccountInfo> = self
            .load_storage_value(storage_map_key(b"System", b"Account", &account.encode()))
            .await?;
        Ok(account_info.map(|info| info.data.free))
    }

    async fn local_peer_id(&self) -> anyhow::Result<String> {
        self.client
            .request("system_localPeerId", rpc_params![])
            .await
            .context("failed to fetch system_localPeerId")
    }

    async fn local_listen_addresses(&self) -> anyhow::Result<Vec<String>> {
        self.client
            .request("system_localListenAddresses", rpc_params![])
            .await
            .context("failed to fetch system_localListenAddresses")
    }

    async fn find_finalized_extrinsic(
        &self,
        extrinsic_hash: H256,
        bytes: &[u8],
    ) -> anyhow::Result<Option<IncludedExtrinsic>> {
        let finalized = self.finalized_number().await?;
        let encoded_hash = H256::from_slice(&sp_core::blake2_256(bytes));
        if encoded_hash != extrinsic_hash {
            bail!(
                "submitted extrinsic hash {} does not match local encoded extrinsic hash {}",
                hex_h256(extrinsic_hash),
                hex_h256(encoded_hash)
            );
        }
        for number in 1..=finalized {
            let Some(hash) = self.block_hash(number).await? else {
                continue;
            };
            let Some(block) = self.block(hash).await? else {
                continue;
            };
            let found = block.block.extrinsics.iter().any(|encoded| {
                decode_hex_bytes(encoded)
                    .map(|raw| H256::from_slice(&sp_core::blake2_256(&raw)) == extrinsic_hash)
                    .unwrap_or(false)
            });
            if found {
                return Ok(Some(IncludedExtrinsic {
                    block_number: number,
                    block_hash: hash,
                }));
            }
        }
        Ok(None)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeVersionView {
    spec_version: u32,
    transaction_version: u32,
}

#[derive(Debug, Deserialize)]
struct RpcHeader {
    number: String,
}

#[derive(Debug, Deserialize)]
struct RpcBlockResponse {
    block: RpcBlock,
}

#[derive(Debug, Deserialize)]
struct RpcBlock {
    extrinsics: Vec<String>,
}

fn build_transfer_extrinsic(
    submitter_pair: &sr25519::Pair,
    account_id: ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    nonce: u32,
    target_domain: DomainId,
    recipient: AccountIdBytes,
    amount: u128,
) -> anyhow::Result<Vec<u8>> {
    let call = ialp_runtime::RuntimeCall::Transfers(TransfersCall::create_cross_domain_transfer {
        target_domain,
        recipient,
        amount,
    });
    let extra = (
        frame_system::CheckNonZeroSender::<ialp_runtime::Runtime>::new(),
        frame_system::CheckSpecVersion::<ialp_runtime::Runtime>::new(),
        frame_system::CheckTxVersion::<ialp_runtime::Runtime>::new(),
        frame_system::CheckGenesis::<ialp_runtime::Runtime>::new(),
        frame_system::CheckEra::<ialp_runtime::Runtime>::from(Era::Immortal),
        frame_system::CheckNonce::<ialp_runtime::Runtime>::from(nonce),
        frame_system::CheckWeight::<ialp_runtime::Runtime>::new(),
        pallet_transaction_payment::ChargeTransactionPayment::<ialp_runtime::Runtime>::from(0u128),
        frame_system::WeightReclaim::<ialp_runtime::Runtime>::new(),
    );
    let implicit = (
        (),
        spec_version,
        transaction_version,
        genesis_hash,
        genesis_hash,
        (),
        (),
        (),
        (),
    );
    let payload = ialp_runtime::SignedPayload::from_raw(call.clone(), extra.clone(), implicit);
    let signature: MultiSignature = submitter_pair.sign(&payload.encode()).into();
    let extrinsic = ialp_runtime::UncheckedExtrinsic::new_signed(
        call,
        MultiAddress::Id(account_id),
        signature,
        extra,
    );
    Ok(extrinsic.encode())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root is two levels up from tests/scenario-harness")
        .to_path_buf()
}

async fn probe_node_runtime_wasm(binary: &Path) -> anyhow::Result<NodeWasmProbe> {
    let config_path = workspace_root()
        .join("config")
        .join("domains")
        .join("earth.toml");
    let output = Command::new(binary)
        .args([
            "build-spec",
            "--domain",
            "earth",
            "--config",
            &display_path(&config_path),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("failed to probe node binary {}", binary.display()))?;

    if output.status.success() {
        return Ok(NodeWasmProbe::Ready);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    if combined.contains("Development wasm not available") {
        return Ok(NodeWasmProbe::MissingDevelopmentWasm);
    }

    bail!(
        "node wasm probe failed for {} with status {:?}: {}",
        binary.display(),
        output.status.code(),
        combined.trim()
    )
}

fn is_workspace_target_binary(binary: &Path) -> bool {
    let target_root = workspace_root().join("target");
    binary.starts_with(target_root)
}

async fn rebuild_workspace_node_binary(binary: &Path) -> anyhow::Result<()> {
    rebuild_workspace_packages(binary, &["ialp-node"]).await
}

async fn rebuild_workspace_scenario_binaries(binary: &Path) -> anyhow::Result<()> {
    rebuild_workspace_packages(
        binary,
        &[
            "ialp-node",
            "ialp-summary-exporter",
            "ialp-summary-relay",
            "ialp-summary-importer",
        ],
    )
    .await
}

async fn rebuild_workspace_packages(binary: &Path, packages: &[&str]) -> anyhow::Result<()> {
    let profile = binary
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("debug");
    let cargo_binary = preferred_workspace_cargo_binary();
    let mut command = Command::new(&cargo_binary);
    command.arg("build").arg("--locked");
    for package in packages {
        command.arg("-p").arg(package);
    }
    if profile == "release" {
        command.arg("--release");
    }
    command.env_remove("SKIP_WASM_BUILD");
    if let Some(toolchain_bin) = preferred_rustup_toolchain_bin_dir() {
        command.env("PATH", prepend_path_entry(&toolchain_bin)?);
    }
    command.current_dir(workspace_root());
    let output = command
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to rebuild ialp-node with runtime wasm enabled")?;
    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "cargo build for workspace scenario packages failed with status {:?}: {}\n{}",
        output.status.code(),
        stdout.trim(),
        stderr.trim()
    )
}

fn preferred_workspace_cargo_binary() -> PathBuf {
    preferred_rustup_toolchain_bin_dir()
        .map(|bin| bin.join(binary_name("cargo")))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

fn preferred_rustup_toolchain_bin_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let rustup_root = PathBuf::from(home).join(".rustup/toolchains");
    for toolchain in ["stable-aarch64-apple-darwin", "1.92.0-aarch64-apple-darwin"] {
        let candidate = rustup_root.join(toolchain).join("bin");
        if candidate.join(binary_name("cargo")).exists()
            && candidate.join(binary_name("rustc")).exists()
        {
            return Some(candidate);
        }
    }
    None
}

fn prepend_path_entry(entry: &Path) -> anyhow::Result<String> {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![entry.to_path_buf()];
    paths.extend(std::env::split_paths(&current));
    std::env::join_paths(paths)
        .context("failed to compose PATH for rustup toolchain override")?
        .into_string()
        .map_err(|_| anyhow!("PATH contains non-utf8 data"))
}

fn artifact_root(base: Option<&Path>, label: &str) -> anyhow::Result<PathBuf> {
    let timestamp = unix_now_millis()?;
    let root = base
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workspace_root().join("target/scenarios"))
        .join(label)
        .join(timestamp.to_string());
    fs::create_dir_all(&root).with_context(|| format!("failed to create {}", root.display()))?;
    Ok(root)
}

fn write_domain_config(
    domain: DomainId,
    ports: &NodePorts,
    layout: &ArtifactLayout,
) -> anyhow::Result<DomainRunConfig> {
    let base_path = workspace_root()
        .join("config")
        .join("domains")
        .join(format!("{}.toml", domain.as_str()));
    let LoadedDomainConfig { mut config, .. } = load_domain_config(domain, Some(&base_path))
        .with_context(|| format!("failed to load base domain config for {}", domain.as_str()))?;
    config.network.rpc_port = ports.rpc_port;
    config.network.p2p_port = ports.p2p_port;
    config.network.prometheus_port = ports.prometheus_port;
    config.epoch.length_seconds = 24;
    // Scenario domains are started explicitly and connected only through harness-controlled
    // bootnodes. Using `live` avoids implicit localhost discovery between distinct domains,
    // which would otherwise happen for `local` chains and create genesis-mismatch noise.
    config.chain_type = DomainChainType::Live;

    let domain_dir = layout.configs_dir.join("domains");
    fs::create_dir_all(&domain_dir)
        .with_context(|| format!("failed to create {}", domain_dir.display()))?;
    let config_path = domain_dir.join(format!("{}.toml", domain.as_str()));
    let contents = toml::to_string_pretty(&config).context("failed to serialize domain config")?;
    fs::write(&config_path, contents)
        .with_context(|| format!("failed to write {}", config_path.display()))?;

    let base_path = layout
        .chains_dir
        .join(format!("{}-authority", domain.as_str()));
    fs::create_dir_all(&base_path)
        .with_context(|| format!("failed to create {}", base_path.display()))?;
    Ok(DomainRunConfig {
        domain,
        config_path,
        rpc_port: ports.rpc_port,
        p2p_port: ports.p2p_port,
        prometheus_port: ports.prometheus_port,
        base_path,
    })
}

fn build_transport_config(
    scenario: ScenarioKind,
    layout: &ArtifactLayout,
    now: DateTime<Utc>,
) -> anyhow::Result<TransportConfig> {
    let ports = allocate_service_ports()?;
    build_transport_config_with_ports(scenario, layout, now, &ports)
}

fn build_transport_config_with_ports(
    scenario: ScenarioKind,
    layout: &ArtifactLayout,
    now: DateTime<Utc>,
    ports: &ServicePorts,
) -> anyhow::Result<TransportConfig> {
    let base_path = workspace_root()
        .join("config")
        .join("transport")
        .join("local.toml");
    let LoadedTransportConfig { mut config, .. } = load_transport_config(Some(&base_path))
        .context("failed to load base transport config for scenario harness")?;
    config.relay.listen_addr = format!("127.0.0.1:{}", ports.relay_port);
    config.relay.store_dir = layout.stores_dir.join("relay");
    config.importers.insert(
        DomainId::Earth,
        ialp_common_config::ImporterTransportConfig {
            listen_addr: format!("127.0.0.1:{}", ports.importer_ports[&DomainId::Earth]),
        },
    );
    config.importers.insert(
        DomainId::Moon,
        ialp_common_config::ImporterTransportConfig {
            listen_addr: format!("127.0.0.1:{}", ports.importer_ports[&DomainId::Moon]),
        },
    );
    config.importers.insert(
        DomainId::Mars,
        ialp_common_config::ImporterTransportConfig {
            listen_addr: format!("127.0.0.1:{}", ports.importer_ports[&DomainId::Mars]),
        },
    );

    for link in &mut config.links {
        if link.source_domain == scenario.source_domain()
            && link.target_domain == scenario.target_domain()
        {
            link.base_one_way_delay_seconds = scenario.link_delay_seconds();
            link.blackout_windows.clear();
            if let Some(window_secs) = scenario.blackout_window_seconds() {
                let start_offset = scenario.blackout_start_offset_seconds().unwrap_or_default();
                let start = now + ChronoDuration::seconds(start_offset as i64);
                link.blackout_windows.push(BlackoutWindowConfig {
                    start,
                    end: start + ChronoDuration::seconds(window_secs as i64),
                });
            }
        } else {
            link.blackout_windows.clear();
        }
    }
    config.validate()?;
    Ok(config)
}

#[derive(Clone)]
struct ServicePorts {
    relay_port: u16,
    importer_ports: BTreeMap<DomainId, u16>,
}

fn allocate_service_ports() -> anyhow::Result<ServicePorts> {
    let relay_port = reserve_port()?.1;
    let earth = reserve_port()?.1;
    let moon = reserve_port()?.1;
    let mars = reserve_port()?.1;
    Ok(ServicePorts {
        relay_port,
        importer_ports: BTreeMap::from([
            (DomainId::Earth, earth),
            (DomainId::Moon, moon),
            (DomainId::Mars, mars),
        ]),
    })
}

fn allocate_node_ports(with_follower: bool) -> anyhow::Result<BTreeMap<String, NodePorts>> {
    let mut map = BTreeMap::new();
    for name in [
        "earth-authority",
        if with_follower { "earth-follower" } else { "" },
        "moon-authority",
        "mars-authority",
    ] {
        if name.is_empty() {
            continue;
        }
        map.insert(
            name.to_string(),
            NodePorts {
                rpc_port: reserve_port()?.1,
                p2p_port: reserve_port()?.1,
                prometheus_port: reserve_port()?.1,
            },
        );
    }
    Ok(map)
}

fn reserve_port() -> anyhow::Result<(TcpListener, u16)> {
    let listener = TcpListener::bind("127.0.0.1:0").context("failed to reserve free TCP port")?;
    let port = listener
        .local_addr()
        .context("reserved listener missing local addr")?
        .port();
    Ok((listener, port))
}

async fn tcp_ready(addr: impl AsRef<str>) -> bool {
    TcpStream::connect(addr.as_ref()).await.is_ok()
}

fn canonical_bootnode(peer_id: &str, addresses: &[String]) -> anyhow::Result<String> {
    let address = addresses
        .iter()
        .find(|addr| addr.contains("/ip4/127.0.0.1/tcp/"))
        .or_else(|| {
            addresses
                .iter()
                .find(|addr| addr.contains("/ip4/0.0.0.0/tcp/"))
        })
        .or_else(|| addresses.first())
        .cloned()
        .ok_or_else(|| anyhow!("authority reported no local listen addresses"))?;
    let mut candidate = address.replace("/ip4/0.0.0.0/", "/ip4/127.0.0.1/");
    if !candidate.contains("/p2p/") {
        candidate.push_str("/p2p/");
        candidate.push_str(peer_id);
    }
    candidate
        .parse::<Multiaddr>()
        .with_context(|| format!("failed to parse derived bootnode multiaddr {}", candidate))?;
    Ok(candidate)
}

fn account_id_bytes_from_seed(seed: &str) -> anyhow::Result<AccountIdBytes> {
    let public = sr25519::Pair::from_string(seed, None)
        .map_err(|error| anyhow!("failed to load account seed {seed}: {error}"))?
        .public();
    Ok(ialp_common_types::fixed_bytes(public.as_ref()))
}

fn account_id_from_seed(seed: &str) -> anyhow::Result<ialp_runtime::AccountId> {
    let pair = sr25519::Pair::from_string(seed, None)
        .map_err(|error| anyhow!("failed to load account seed {seed}: {error}"))?;
    Ok(submitter_account_id(&pair))
}

fn submitter_account_id(pair: &sr25519::Pair) -> ialp_runtime::AccountId {
    <ialp_runtime::Signature as Verify>::Signer::from(pair.public()).into_account()
}

fn seed_flag(seed: &str) -> &'static str {
    match seed {
        "//Alice" => "--alice",
        "//Bob" => "--bob",
        "//Charlie" => "--charlie",
        "//Dave" => "--dave",
        other => panic!("unsupported dev seed flag {other}"),
    }
}

fn port_from_listen_addr(addr: &str) -> anyhow::Result<u16> {
    let (_, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("listen addr {} is missing a port", addr))?;
    port.parse::<u16>()
        .with_context(|| format!("failed to parse port from {}", addr))
}

fn parse_h256(value: &str) -> anyhow::Result<H256> {
    let bytes = decode_hex_bytes(value)?;
    Ok(H256::from_slice(&bytes))
}

fn parse_hex_u32(value: &str) -> anyhow::Result<u32> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    u32::from_str_radix(trimmed, 16)
        .with_context(|| format!("failed to parse hex block number {}", value))
}

fn decode_hex_bytes(value: &str) -> anyhow::Result<Vec<u8>> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(trimmed).with_context(|| format!("failed to decode hex {}", value))
}

fn hex_hash(hash: [u8; 32]) -> String {
    format!("0x{}", hex::encode(hash))
}

fn hex_h256(hash: H256) -> String {
    format!("0x{}", hex::encode(hash.as_bytes()))
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

fn elapsed_millis(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn unix_now_millis() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_millis()
        .try_into()
        .context("unix timestamp overflow")
}

fn binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn relay_state_name(state: &ialp_summary_relay::store::RelayQueueState) -> &'static str {
    match state {
        ialp_summary_relay::store::RelayQueueState::Queued => "queued",
        ialp_summary_relay::store::RelayQueueState::Scheduled => "scheduled",
        ialp_summary_relay::store::RelayQueueState::BlockedByBlackout => "blocked_by_blackout",
        ialp_summary_relay::store::RelayQueueState::InDelivery => "in_delivery",
        ialp_summary_relay::store::RelayQueueState::Delivered => "delivered",
        ialp_summary_relay::store::RelayQueueState::ImporterAcked => "importer_acked",
        ialp_summary_relay::store::RelayQueueState::Retrying => "retrying",
        ialp_summary_relay::store::RelayQueueState::Failed => "failed",
    }
}

fn print_summary(summary: &ScenarioSummary, _json: bool) {
    println!(
        "{}",
        serde_json::to_string_pretty(summary).expect("summary should serialize")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scenario_transport_config_is_serializable_and_valid() {
        let temp = TempDir::new().expect("tempdir");
        let layout = ArtifactLayout::new(temp.path().join("artifacts")).expect("layout");
        let ports = ServicePorts {
            relay_port: 43000,
            importer_ports: BTreeMap::from([
                (DomainId::Earth, 43001),
                (DomainId::Moon, 43002),
                (DomainId::Mars, 43003),
            ]),
        };
        let config = build_transport_config_with_ports(
            ScenarioKind::EarthToMarsBlackout,
            &layout,
            Utc::now(),
            &ports,
        )
        .expect("transport config");
        assert_eq!(
            config
                .link(DomainId::Earth, DomainId::Mars)
                .expect("earth->mars link")
                .base_one_way_delay_seconds,
            2
        );
        assert_eq!(
            config
                .link(DomainId::Earth, DomainId::Mars)
                .expect("earth->mars link")
                .blackout_windows
                .len(),
            1
        );
        config.validate().expect("transport config should validate");
    }

    #[test]
    fn domain_config_override_sets_epoch_and_ports() {
        let temp = TempDir::new().expect("tempdir");
        let layout = ArtifactLayout::new(temp.path().join("artifacts")).expect("layout");
        let ports = NodePorts {
            rpc_port: 42001,
            p2p_port: 42002,
            prometheus_port: 42003,
        };
        let written = write_domain_config(DomainId::Earth, &ports, &layout).expect("write config");
        let loaded =
            load_domain_config(DomainId::Earth, Some(&written.config_path)).expect("load config");
        assert_eq!(loaded.config.network.rpc_port, 42001);
        assert_eq!(loaded.config.network.p2p_port, 42002);
        assert_eq!(loaded.config.network.prometheus_port, 42003);
        assert_eq!(loaded.config.epoch.length_seconds, 24);
    }

    #[test]
    fn summary_json_schema_contains_expected_fields() {
        let summary = ScenarioSummary {
            schema_version: SUMMARY_SCHEMA_VERSION,
            scenario: ScenarioKind::EarthToMoonSuccess,
            success: true,
            started_at_unix_ms: 1,
            ended_at_unix_ms: Some(2),
            failed_stage: None,
            failure_message: None,
            source_domain: DomainId::Earth,
            target_domain: DomainId::Moon,
            source_epoch_id: Some(3),
            extrinsic_hash: Some("0x01".into()),
            export_ids: vec!["0x02".into()],
            summary_hash: Some("0x03".into()),
            package_hash: Some("0x04".into()),
            completion_package_hash: Some("0x05".into()),
            final_relay_state: Some("importer_acked".into()),
            final_importer_package_state: Some(
                ialp_common_types::ImporterPackageState::AckedVerified,
            ),
            destination_observation: None,
            stage_results: vec![StageResult {
                stage: "node_readiness".into(),
                success: true,
                timed_out: false,
                duration_ms: 100,
                timeout_seconds: None,
                details: json!({"ready": true}),
            }],
            artifact_paths: ArtifactPaths {
                root: "/tmp/root".into(),
                configs_dir: "/tmp/configs".into(),
                chains_dir: "/tmp/chains".into(),
                stores_dir: "/tmp/stores".into(),
                logs_dir: "/tmp/logs".into(),
                summary_json: "/tmp/summary.json".into(),
            },
        };
        let value = serde_json::to_value(&summary).expect("summary json");
        assert_eq!(value["schema_version"], json!(1));
        assert_eq!(value["scenario"], json!("earth-to-moon-success"));
        assert_eq!(
            value["artifact_paths"]["summary_json"],
            json!("/tmp/summary.json")
        );
    }

    #[test]
    fn canonical_bootnode_appends_peer_id_and_rewrites_localhost() {
        let peer_id = "12D3KooWRf5X9gC7yJw6m1CEk6P4F5Y5L3yqQ4cjQUh1SVfMpo1e";
        let bootnode = canonical_bootnode(peer_id, &[String::from("/ip4/0.0.0.0/tcp/30333")])
            .expect("bootnode");
        assert!(bootnode.contains("/ip4/127.0.0.1/tcp/30333"));
        assert!(bootnode.ends_with(peer_id));
    }
}
