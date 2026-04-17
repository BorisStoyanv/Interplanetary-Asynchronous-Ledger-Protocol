mod cli;
mod rpc_client;
pub mod store;

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context};
use axum::{
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use codec::{Decode, Encode};
use ialp_common_config::{load_domain_config, load_transport_config};
use ialp_common_types::{
    summary_header_storage_key, CertifiedSummaryPackage, DomainId, FinalizedImportInclusionProof,
    GovernanceInclusionProof, GovernanceLeaf, ImportedGovernanceAckClaim,
    ImportedGovernanceProposalClaim, ImporterPackageState, ImporterPackageStatusView,
    InclusionProof, ObservedImportClaim, RelayPackageEnvelopeV1, RemoteFinalizationClaim,
    SummaryCertificate,
};
use pallet_ialp_governance::Call as GovernanceCall;
use pallet_ialp_transfers::Call as TransfersCall;
use sc_consensus_grandpa::GrandpaJustification;
use sp_consensus_grandpa::AuthorityId as GrandpaId;
use sp_core::{crypto::Ss58Codec, sr25519, Pair, Public, H256};
use sp_runtime::{
    generic::Era,
    traits::{BlakeTwo256, Header as HeaderT, IdentifyAccount, Verify},
    MultiAddress, MultiSignature,
};
use sp_state_machine::{read_proof_check, StorageProof};
use tokio::{net::TcpListener, sync::Mutex, time::sleep};

use crate::{
    cli::{parse_export_id_hex, Cli, Commands},
    rpc_client::{load_submitter_pair, NodeRpcClient},
    store::{
        decode_hex_hash, DuplicateStatus, ImportRecord, ImporterIndex, PackageRecord, Store,
        SubmissionStatus, VerificationStatus,
    },
};

const PROCESSOR_TICK_MILLIS: u64 = 500;

#[derive(Clone)]
struct ImporterSettings {
    domain: DomainId,
    node_url: String,
    listen_addr: Option<String>,
    store: Arc<Store>,
    submitter_suri: Option<String>,
}

#[derive(Clone)]
struct AppState {
    settings: ImporterSettings,
    index: Arc<Mutex<ImporterIndex>>,
}

#[derive(Debug, serde::Serialize)]
struct ImporterIngestReceipt {
    accepted: bool,
    idempotent: bool,
    status: ImporterPackageStatusView,
}

struct VerifiedPackageItems {
    export_proofs: Vec<ialp_common_types::ExportInclusionProof>,
    finalized_import_proofs: Vec<FinalizedImportInclusionProof>,
    governance_proofs: Vec<GovernanceInclusionProof>,
}

pub async fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => run_importer(args).await,
        Commands::Verify(args) => verify_package_command(args).await,
        Commands::Status(args) => show_status(args),
        Commands::Show(args) => show_record(args),
    }
}

async fn run_importer(args: cli::RunArgs) -> anyhow::Result<()> {
    let settings = ImporterSettings::load(
        args.domain,
        args.node_url,
        args.store_dir,
        args.transport_config,
        Some(args.submitter_suri),
    )?;
    let index = settings.store.load_index()?;
    let app_state = Arc::new(AppState {
        settings: settings.clone(),
        index: Arc::new(Mutex::new(index)),
    });

    let processor_state = app_state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = process_pending_packages_once(&processor_state).await {
                eprintln!("importer processing loop failed: {error:#}");
            }
            sleep(Duration::from_millis(PROCESSOR_TICK_MILLIS)).await;
        }
    });

    let listen_addr = settings
        .listen_addr
        .clone()
        .ok_or_else(|| anyhow!("importer run mode requires a transport config with listen_addr"))?;
    let router = Router::new()
        .route("/api/v1/packages", post(receive_package))
        .route(
            "/api/v1/packages/{source_domain}/{target_domain}/{epoch_id}/{package_hash}",
            get(package_status),
        )
        .with_state(app_state);
    let listener = TcpListener::bind(&listen_addr)
        .await
        .with_context(|| format!("failed to bind importer listener {}", listen_addr))?;
    axum::serve(listener, router)
        .await
        .context("importer HTTP server exited unexpectedly")
}

async fn verify_package_command(args: cli::VerifyArgs) -> anyhow::Result<()> {
    let settings = ImporterSettings::load(
        args.domain,
        args.node_url,
        args.store_dir,
        args.transport_config,
        Some(args.submitter_suri),
    )?;
    let package_bytes = std::fs::read(&args.package)
        .with_context(|| format!("failed to read package {}", args.package.display()))?;
    let package = CertifiedSummaryPackage::decode(&mut &package_bytes[..])
        .map_err(|error| anyhow!("failed to decode certified summary package: {error}"))?;
    verify_package_hash(&package)?;
    let package_view = inspect_package_for_ingest(&package, settings.domain)?;
    let now = unix_now_millis()?;
    let mut index = settings.store.load_index()?;
    let (record, _) = ensure_package_record(
        &settings.store,
        &mut index,
        package_view.source_domain,
        package_view.target_domain,
        package_view.epoch_id,
        package.header.summary_hash,
        package.package_hash,
        package_view.export_count,
        now,
        &package_bytes,
    )?;
    settings.store.save_index(&index)?;
    process_package_by_identity(
        &settings,
        record.source_domain,
        record.target_domain,
        record.epoch_id,
        decode_hex_hash(&record.package_hash)?,
    )
    .await?;

    let refreshed_index = settings.store.load_index()?;
    let package_record = refreshed_index
        .package(
            package_view.source_domain,
            package_view.target_domain,
            package_view.epoch_id,
            package.package_hash,
        )
        .cloned()
        .ok_or_else(|| anyhow!("package record missing after verification"))?;

    let output = serde_json::json!({
        "domain": args.domain,
        "package": args.package,
        "status": package_status_view(&package_record),
        "imports": refreshed_index.imports.iter().map(ImportRecord::json_summary).collect::<Vec<_>>(),
    });
    render_json(output, args.json);
    Ok(())
}

fn show_status(args: cli::StatusArgs) -> anyhow::Result<()> {
    let settings = ImporterSettings::load(args.domain, None, args.store_dir, None, None)?;
    let index = settings.store.load_index()?;

    let output = if let Some(export_id) = args.export_id {
        let export_id = parse_export_id_hex(&export_id)?;
        serde_json::json!({
            "domain": args.domain,
            "record": index.record(export_id).map(ImportRecord::json_summary),
        })
    } else {
        serde_json::json!({
            "domain": args.domain,
            "index": index.json_summary(),
            "packages": index.packages.iter().map(PackageRecord::json_summary).collect::<Vec<_>>(),
            "records": index.imports.iter().map(ImportRecord::json_summary).collect::<Vec<_>>(),
        })
    };
    render_json(output, args.json);
    Ok(())
}

fn show_record(args: cli::ShowArgs) -> anyhow::Result<()> {
    let settings = ImporterSettings::load(args.domain, None, args.store_dir, None, None)?;
    let export_id = parse_export_id_hex(&args.export_id)?;
    let record = settings
        .store
        .load_record(export_id)?
        .ok_or_else(|| anyhow!("no importer record found for export_id {}", args.export_id))?;
    render_json(record.json_summary(), args.json);
    Ok(())
}

async fn receive_package(State(app): State<Arc<AppState>>, body: Bytes) -> impl IntoResponse {
    let response = async {
        let envelope = RelayPackageEnvelopeV1::decode(&mut &body[..])
            .map_err(|error| anyhow!("failed to decode relay package envelope: {error}"))?;
        let package = CertifiedSummaryPackage::decode(&mut &envelope.package_bytes[..])
            .map_err(|error| anyhow!("failed to decode certified summary package: {error}"))?;
        verify_package_hash(&package)?;
        let package_view = inspect_package_for_ingest(&package, app.settings.domain)?;
        if envelope.source_domain != package_view.source_domain
            || envelope.target_domain != package_view.target_domain
            || envelope.epoch_id != package_view.epoch_id
            || envelope.summary_hash != package.header.summary_hash
            || envelope.package_hash != package.package_hash
            || envelope.export_count != package_view.export_count
        {
            bail!("relay envelope metadata does not match certified package contents");
        }

        let now = unix_now_millis()?;
        let mut index = app.index.lock().await;
        let (record, _idempotent) = ensure_package_record(
            &app.settings.store,
            &mut index,
            package_view.source_domain,
            package_view.target_domain,
            package_view.epoch_id,
            package.header.summary_hash,
            package.package_hash,
            package_view.export_count,
            now,
            &envelope.package_bytes,
        )?;
        app.settings.store.save_index(&index)?;
        let idempotent = record.received_at_unix_ms != now;
        let status = if idempotent {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        Ok::<_, anyhow::Error>((
            status,
            Json(ImporterIngestReceipt {
                accepted: true,
                idempotent,
                status: package_status_view(&record),
            }),
        ))
    }
    .await;

    match response {
        Ok(success) => success.into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

async fn package_status(
    State(app): State<Arc<AppState>>,
    AxumPath((source_domain, target_domain, epoch_id, package_hash)): AxumPath<(
        DomainId,
        DomainId,
        u64,
        String,
    )>,
) -> impl IntoResponse {
    let response = async {
        let package_hash = decode_hex_hash(&package_hash)?;
        let index = app.index.lock().await;
        let record = index
            .package(source_domain, target_domain, epoch_id, package_hash)
            .ok_or_else(|| anyhow!("importer package not found"))?;
        Ok::<_, anyhow::Error>(Json(package_status_view(record)))
    }
    .await;

    match response {
        Ok(success) => (StatusCode::OK, success).into_response(),
        Err(error) if error.to_string().contains("not found") => {
            (StatusCode::NOT_FOUND, error.to_string()).into_response()
        }
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

async fn process_pending_packages_once(app: &Arc<AppState>) -> anyhow::Result<()> {
    let work = {
        let index = app.index.lock().await;
        index
            .packages
            .iter()
            .filter(|record| !record.state.is_terminal())
            .map(|record| {
                (
                    record.source_domain,
                    record.target_domain,
                    record.epoch_id,
                    record.package_hash.clone(),
                )
            })
            .collect::<Vec<_>>()
    };

    for (source_domain, target_domain, epoch_id, package_hash) in work {
        process_package_by_identity(
            &app.settings,
            source_domain,
            target_domain,
            epoch_id,
            decode_hex_hash(&package_hash)?,
        )
        .await?;
        let refreshed = app.settings.store.load_index()?;
        let mut index = app.index.lock().await;
        *index = refreshed;
    }

    Ok(())
}

async fn process_package_by_identity(
    settings: &ImporterSettings,
    source_domain: DomainId,
    target_domain: DomainId,
    epoch_id: u64,
    package_hash: [u8; 32],
) -> anyhow::Result<()> {
    let mut index = settings.store.load_index()?;
    let mut package_record = index
        .package(source_domain, target_domain, epoch_id, package_hash)
        .cloned()
        .ok_or_else(|| anyhow!("importer package record not found"))?;

    if package_record.state.is_terminal() {
        return Ok(());
    }

    package_record.state = ImporterPackageState::Verifying;
    package_record.last_updated_at_unix_ms = unix_now_millis()?;
    package_record.reason = None;
    settings
        .store
        .persist_package(&mut index, package_record.clone(), None)?;
    settings.store.save_index(&index)?;

    let payload = settings.store.load_package_bytes(&package_record)?;
    let package = match CertifiedSummaryPackage::decode(&mut &payload[..]) {
        Ok(package) => package,
        Err(error) => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::AckedInvalid,
                format!("failed to decode stored package bytes: {error}"),
            )?;
            return Ok(());
        }
    };

    if let Err(error) = verify_package_hash(&package) {
        finalize_package_error(
            settings,
            package_record,
            ImporterPackageState::AckedInvalid,
            error.to_string(),
        )?;
        return Ok(());
    }

    let certificate = match verify_summary_chain_context(&package) {
        Ok(certificate) => certificate,
        Err(error) => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::AckedInvalid,
                error.to_string(),
            )?;
            return Ok(());
        }
    };
    if let Err(error) = verify_grandpa_certificate(&certificate, package.header.domain_id) {
        finalize_package_error(
            settings,
            package_record,
            ImporterPackageState::AckedInvalid,
            error.to_string(),
        )?;
        return Ok(());
    }
    let package_items = match verify_package_flow(&package, settings.domain) {
        Ok(items) => items,
        Err(error) => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::AckedInvalid,
                error.to_string(),
            )?;
            return Ok(());
        }
    };

    let rpc = match NodeRpcClient::connect(&settings.node_url).await {
        Ok(rpc) => rpc,
        Err(error) => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::SubmissionRetrying,
                error.to_string(),
            )?;
            return Ok(());
        }
    };
    let Some(submitter_suri) = settings.submitter_suri.as_deref() else {
        finalize_package_error(
            settings,
            package_record,
            ImporterPackageState::AckedInvalid,
            "submitter_suri is required for importer processing".into(),
        )?;
        return Ok(());
    };
    let submitter_pair = match load_submitter_pair(submitter_suri) {
        Ok(pair) => pair,
        Err(error) => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::AckedInvalid,
                error.to_string(),
            )?;
            return Ok(());
        }
    };
    let submitter_account = submitter_account_id(&submitter_pair);
    let configured_importer = match rpc.importer_account().await? {
        Some(account) => account,
        None => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::SubmissionRetrying,
                "destination chain is missing configured importer account".into(),
            )?;
            return Ok(());
        }
    };
    if submitter_account != configured_importer {
        finalize_package_error(
            settings,
            package_record,
            ImporterPackageState::AckedInvalid,
            format!(
                "submitter account {} does not match allowlisted importer account {}",
                submitter_account.to_ss58check(),
                configured_importer.to_ss58check()
            ),
        )?;
        return Ok(());
    }
    if !package_items.governance_proofs.is_empty() {
        let governance_importer = match rpc.governance_importer_account().await? {
            Some(account) => account,
            None => {
                finalize_package_error(
                    settings,
                    package_record,
                    ImporterPackageState::SubmissionRetrying,
                    "destination chain is missing configured governance importer account".into(),
                )?;
                return Ok(());
            }
        };
        if submitter_account != governance_importer {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::AckedInvalid,
                format!(
                    "submitter account {} does not match allowlisted governance importer account {}",
                    submitter_account.to_ss58check(),
                    governance_importer.to_ss58check()
                ),
            )?;
            return Ok(());
        }
    }

    let runtime_version = match rpc.runtime_version().await {
        Ok(runtime_version) => runtime_version,
        Err(error) => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::SubmissionRetrying,
                error.to_string(),
            )?;
            return Ok(());
        }
    };
    if let Err(error) = ensure_runtime_version_matches(&runtime_version) {
        finalize_package_error(
            settings,
            package_record,
            ImporterPackageState::AckedInvalid,
            error.to_string(),
        )?;
        return Ok(());
    }
    let genesis_hash = match rpc.genesis_hash().await {
        Ok(hash) => hash,
        Err(error) => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::SubmissionRetrying,
                error.to_string(),
            )?;
            return Ok(());
        }
    };

    let outcome = process_package_items(
        settings,
        &rpc,
        &submitter_pair,
        &submitter_account,
        runtime_version.spec_version,
        runtime_version.transaction_version,
        genesis_hash,
        &package,
        package_items,
    )
    .await;

    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(ProcessPackageError::Retryable(message)) => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::SubmissionRetrying,
                message,
            )?;
            return Ok(());
        }
        Err(ProcessPackageError::Invalid(message)) => {
            finalize_package_error(
                settings,
                package_record,
                ImporterPackageState::AckedInvalid,
                message,
            )?;
            return Ok(());
        }
    };

    let mut refreshed_package = settings
        .store
        .load_package(source_domain, target_domain, epoch_id, package_hash)?
        .ok_or_else(|| anyhow!("package disappeared during processing"))?;
    refreshed_package.state = outcome.final_state;
    refreshed_package.tx_hashes = outcome.tx_hashes;
    refreshed_package.completed_at_unix_ms = Some(unix_now_millis()?);
    refreshed_package.last_updated_at_unix_ms = refreshed_package.completed_at_unix_ms.unwrap();
    refreshed_package.reason = Some(outcome.reason);
    settings
        .store
        .persist_package(&mut index, refreshed_package, None)?;
    settings.store.save_index(&index)?;
    Ok(())
}

fn ensure_package_record(
    store: &Store,
    index: &mut ImporterIndex,
    source_domain: DomainId,
    target_domain: DomainId,
    epoch_id: u64,
    summary_hash: [u8; 32],
    package_hash: [u8; 32],
    export_count: u32,
    now: u64,
    payload_bytes: &[u8],
) -> anyhow::Result<(PackageRecord, bool)> {
    if let Some(existing) = index
        .package(source_domain, target_domain, epoch_id, package_hash)
        .cloned()
    {
        store.persist_package(index, existing.clone(), Some(payload_bytes))?;
        return Ok((existing, true));
    }

    let record = store.build_package_record(
        source_domain,
        target_domain,
        epoch_id,
        summary_hash,
        package_hash,
        export_count,
        now,
    );
    store.persist_package(index, record.clone(), Some(payload_bytes))?;
    Ok((record, false))
}

fn finalize_package_error(
    settings: &ImporterSettings,
    mut record: PackageRecord,
    state: ImporterPackageState,
    reason: String,
) -> anyhow::Result<()> {
    let mut index = settings.store.load_index()?;
    record.state = state;
    record.reason = Some(reason);
    record.last_updated_at_unix_ms = unix_now_millis()?;
    if state.is_terminal() {
        record.completed_at_unix_ms = Some(record.last_updated_at_unix_ms);
    }
    settings.store.persist_package(&mut index, record, None)?;
    settings.store.save_index(&index)
}

enum ProcessPackageError {
    Retryable(String),
    Invalid(String),
}

struct ProcessPackageOutcome {
    final_state: ImporterPackageState,
    tx_hashes: Vec<String>,
    reason: String,
}

async fn process_package_items(
    settings: &ImporterSettings,
    rpc: &NodeRpcClient,
    submitter_pair: &sr25519::Pair,
    submitter_account: &ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    package: &CertifiedSummaryPackage,
    items: VerifiedPackageItems,
) -> Result<ProcessPackageOutcome, ProcessPackageError> {
    let mut outcomes = Vec::new();

    if !items.export_proofs.is_empty() {
        outcomes.push(
            process_forward_exports(
                settings,
                rpc,
                submitter_pair,
                submitter_account,
                spec_version,
                transaction_version,
                genesis_hash,
                package,
                items.export_proofs,
            )
            .await?,
        );
    }

    if !items.finalized_import_proofs.is_empty() {
        outcomes.push(
            process_settlement_completions(
                settings,
                rpc,
                submitter_pair,
                submitter_account,
                spec_version,
                transaction_version,
                genesis_hash,
                package,
                items.finalized_import_proofs,
            )
            .await?,
        );
    }

    if !items.governance_proofs.is_empty() {
        outcomes.push(
            process_governance_proofs(
                settings,
                rpc,
                submitter_pair,
                submitter_account,
                spec_version,
                transaction_version,
                genesis_hash,
                package,
                items.governance_proofs,
            )
            .await?,
        );
    }

    if outcomes.is_empty() {
        return Err(ProcessPackageError::Invalid(
            "package does not contain any actionable proofs".into(),
        ));
    }

    let mut tx_hashes = Vec::new();
    let mut reasons = Vec::new();
    let mut any_verified = false;
    let mut all_duplicate_local = true;
    let mut all_duplicate_remote = true;
    for outcome in outcomes {
        tx_hashes.extend(outcome.tx_hashes);
        reasons.push(outcome.reason);
        match outcome.final_state {
            ImporterPackageState::AckedVerified => {
                any_verified = true;
                all_duplicate_local = false;
                all_duplicate_remote = false;
            }
            ImporterPackageState::AckedDuplicateLocal => {
                all_duplicate_remote = false;
            }
            ImporterPackageState::AckedDuplicateRemote => {
                all_duplicate_local = false;
            }
            _ => {
                any_verified = true;
                all_duplicate_local = false;
                all_duplicate_remote = false;
            }
        }
    }

    let final_state = if any_verified {
        ImporterPackageState::AckedVerified
    } else if all_duplicate_local {
        ImporterPackageState::AckedDuplicateLocal
    } else if all_duplicate_remote {
        ImporterPackageState::AckedDuplicateRemote
    } else {
        ImporterPackageState::AckedVerified
    };

    Ok(ProcessPackageOutcome {
        final_state,
        tx_hashes,
        reason: reasons.join("; "),
    })
}

async fn process_forward_exports(
    settings: &ImporterSettings,
    rpc: &NodeRpcClient,
    submitter_pair: &sr25519::Pair,
    submitter_account: &ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    package: &CertifiedSummaryPackage,
    export_proofs: Vec<ialp_common_types::ExportInclusionProof>,
) -> Result<ProcessPackageOutcome, ProcessPackageError> {
    let mut index = settings
        .store
        .load_index()
        .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
    let mut tx_hashes = Vec::new();
    let mut duplicate_local = 0usize;
    let mut duplicate_remote = 0usize;
    let mut finalized = 0usize;

    for proof in export_proofs {
        let export_id = proof.leaf.export_id;
        if index.record(export_id).is_some() {
            duplicate_local = duplicate_local.saturating_add(1);
            continue;
        }

        let existing_remote = rpc
            .observed_import(export_id)
            .await
            .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
        if let Some(ref record) = existing_remote {
            if record.status == ialp_common_types::ImportObservationStatus::RemoteFinalized {
                duplicate_remote = duplicate_remote.saturating_add(1);
                settings
                    .store
                    .persist_record(
                        &mut index,
                        ImportRecord {
                            export_id: hex_hash(export_id),
                            source_domain: proof.leaf.source_domain,
                            target_domain: proof.leaf.target_domain,
                            summary_hash: hex_hash(package.header.summary_hash),
                            package_hash: hex_hash(package.package_hash),
                            verification_status: VerificationStatus::Verified,
                            duplicate_status: DuplicateStatus::DuplicateRemote,
                            submission_status: SubmissionStatus::RemoteFinalized,
                            tx_hash: None,
                            reason: Some("export_id already remote_finalized on destination chain".into()),
                        },
                    )
                    .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
                continue;
            }
        }

        let mut next_nonce = rpc
            .account_next_index(submitter_account)
            .await
            .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;

        if existing_remote.is_none() {
            let claim = ObservedImportClaim {
                version: ialp_common_types::OBSERVED_IMPORT_VERSION,
                export_id: proof.leaf.export_id,
                source_domain: proof.leaf.source_domain,
                target_domain: proof.leaf.target_domain,
                source_epoch_id: proof.leaf.source_epoch_id,
                summary_hash: package.header.summary_hash,
                package_hash: package.package_hash,
                recipient: proof.leaf.recipient,
                amount: proof.leaf.amount,
            };
            let extrinsic = build_observed_import_extrinsic(
                submitter_pair,
                submitter_account.clone(),
                spec_version,
                transaction_version,
                genesis_hash,
                next_nonce,
                claim,
            )
            .map_err(|error| ProcessPackageError::Invalid(error.to_string()))?;
            let tx_hash = rpc
                .submit_extrinsic(extrinsic)
                .await
                .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
            tx_hashes.push(tx_hash);
            next_nonce = next_nonce.saturating_add(1);
        }

        let finalize_extrinsic = build_finalize_verified_import_extrinsic(
            submitter_pair,
            submitter_account.clone(),
            spec_version,
            transaction_version,
            genesis_hash,
            next_nonce,
            export_id,
        )
        .map_err(|error| ProcessPackageError::Invalid(error.to_string()))?;
        let finalize_tx = rpc
            .submit_extrinsic(finalize_extrinsic)
            .await
            .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
        tx_hashes.push(finalize_tx.clone());

        wait_for_observed_import_status(
            rpc,
            export_id,
            ialp_common_types::ImportObservationStatus::RemoteFinalized,
        )
        .await
        .map_err(ProcessPackageError::Retryable)?;

        settings
            .store
            .persist_record(
                &mut index,
                ImportRecord {
                    export_id: hex_hash(export_id),
                    source_domain: proof.leaf.source_domain,
                    target_domain: proof.leaf.target_domain,
                    summary_hash: hex_hash(package.header.summary_hash),
                    package_hash: hex_hash(package.package_hash),
                    verification_status: VerificationStatus::Verified,
                    duplicate_status: DuplicateStatus::NotDuplicate,
                    submission_status: SubmissionStatus::RemoteFinalized,
                    tx_hash: Some(finalize_tx),
                    reason: Some("remote_finalized recorded".into()),
                },
            )
            .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
        finalized = finalized.saturating_add(1);
    }

    settings
        .store
        .save_index(&index)
        .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;

    let final_state = if finalized > 0 {
        ImporterPackageState::AckedVerified
    } else if duplicate_local > 0 && duplicate_remote == 0 {
        ImporterPackageState::AckedDuplicateLocal
    } else if duplicate_remote > 0 && duplicate_local == 0 {
        ImporterPackageState::AckedDuplicateRemote
    } else {
        ImporterPackageState::AckedVerified
    };

    let reason = match final_state {
        ImporterPackageState::AckedVerified => {
            "package verified and remote_finalized recorded or already known".into()
        }
        ImporterPackageState::AckedDuplicateLocal => "all exports already processed locally".into(),
        ImporterPackageState::AckedDuplicateRemote => {
            "all exports already remote_finalized on destination chain".into()
        }
        _ => "package processing completed".into(),
    };

    Ok(ProcessPackageOutcome {
        final_state,
        tx_hashes,
        reason,
    })
}

async fn process_settlement_completions(
    settings: &ImporterSettings,
    rpc: &NodeRpcClient,
    submitter_pair: &sr25519::Pair,
    submitter_account: &ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    package: &CertifiedSummaryPackage,
    finalized_import_proofs: Vec<FinalizedImportInclusionProof>,
) -> Result<ProcessPackageOutcome, ProcessPackageError> {
    let mut index = settings
        .store
        .load_index()
        .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
    let mut tx_hashes = Vec::new();
    let mut duplicate_local = 0usize;
    let mut resolved = 0usize;

    for proof in finalized_import_proofs {
        let export_id = proof.leaf.export_id;
        if index.record(export_id).is_some() {
            duplicate_local = duplicate_local.saturating_add(1);
            continue;
        }

        let existing_export = rpc
            .export_record(export_id)
            .await
            .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?
            .ok_or_else(|| {
                ProcessPackageError::Invalid(format!(
                    "source chain is missing export record 0x{}",
                    hex::encode(export_id)
                ))
            })?;
        if existing_export.status == ialp_common_types::ExportStatus::RemoteFinalized {
            settings
                .store
                .persist_record(
                    &mut index,
                    ImportRecord {
                        export_id: hex_hash(export_id),
                        source_domain: proof.leaf.source_domain,
                        target_domain: proof.leaf.target_domain,
                        summary_hash: hex_hash(package.header.summary_hash),
                        package_hash: hex_hash(package.package_hash),
                        verification_status: VerificationStatus::Verified,
                        duplicate_status: DuplicateStatus::DuplicateRemote,
                        submission_status: SubmissionStatus::SourceResolved,
                        tx_hash: None,
                        reason: Some("source export already remote_finalized".into()),
                    },
                )
                .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
            continue;
        }

        let claim = RemoteFinalizationClaim {
            version: ialp_common_types::REMOTE_FINALIZATION_CLAIM_VERSION,
            export_id,
            source_domain: proof.leaf.source_domain,
            target_domain: proof.leaf.target_domain,
            source_epoch_id: proof.leaf.source_epoch_id,
            recipient: proof.leaf.recipient,
            amount: proof.leaf.amount,
            completion_summary_hash: package.header.summary_hash,
            completion_package_hash: package.package_hash,
        };
        let nonce = rpc
            .account_next_index(submitter_account)
            .await
            .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
        let extrinsic = build_acknowledge_remote_finalization_extrinsic(
            submitter_pair,
            submitter_account.clone(),
            spec_version,
            transaction_version,
            genesis_hash,
            nonce,
            claim,
        )
        .map_err(|error| ProcessPackageError::Invalid(error.to_string()))?;
        let tx_hash = rpc
            .submit_extrinsic(extrinsic)
            .await
            .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;

        wait_for_export_record_status(
            rpc,
            export_id,
            ialp_common_types::ExportStatus::RemoteFinalized,
        )
        .await
        .map_err(ProcessPackageError::Retryable)?;

        settings
            .store
            .persist_record(
                &mut index,
                ImportRecord {
                    export_id: hex_hash(export_id),
                    source_domain: proof.leaf.source_domain,
                    target_domain: proof.leaf.target_domain,
                    summary_hash: hex_hash(package.header.summary_hash),
                    package_hash: hex_hash(package.package_hash),
                    verification_status: VerificationStatus::Verified,
                    duplicate_status: DuplicateStatus::NotDuplicate,
                    submission_status: SubmissionStatus::SourceResolved,
                    tx_hash: Some(tx_hash.clone()),
                    reason: Some("source hold resolved from certified remote_finalized proof".into()),
                },
            )
            .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
        tx_hashes.push(tx_hash);
        resolved = resolved.saturating_add(1);
    }

    settings
        .store
        .save_index(&index)
        .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;

    let final_state = if resolved > 0 {
        ImporterPackageState::AckedVerified
    } else if duplicate_local > 0 {
        ImporterPackageState::AckedDuplicateLocal
    } else {
        ImporterPackageState::AckedDuplicateRemote
    };
    let reason = match final_state {
        ImporterPackageState::AckedVerified => {
            "completion package verified and source holds resolved".into()
        }
        ImporterPackageState::AckedDuplicateLocal => {
            "all completion proofs already processed locally".into()
        }
        ImporterPackageState::AckedDuplicateRemote => {
            "all source exports were already remote_finalized".into()
        }
        _ => "package processing completed".into(),
    };

    Ok(ProcessPackageOutcome {
        final_state,
        tx_hashes,
        reason,
    })
}

async fn process_governance_proofs(
    _settings: &ImporterSettings,
    rpc: &NodeRpcClient,
    submitter_pair: &sr25519::Pair,
    submitter_account: &ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    package: &CertifiedSummaryPackage,
    governance_proofs: Vec<GovernanceInclusionProof>,
) -> Result<ProcessPackageOutcome, ProcessPackageError> {
    let mut tx_hashes = Vec::new();
    let mut imported = 0usize;
    let mut duplicate_remote = 0usize;

    for proof in governance_proofs {
        match proof.leaf {
            GovernanceLeaf::ProposalV1(leaf) => {
                let proposal_id = leaf.proposal_id;
                if let Some(existing) = rpc
                    .governance_proposal(proposal_id)
                    .await
                    .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?
                {
                    if existing.source_domain != leaf.source_domain
                        || existing.target_domains != leaf.target_domains
                        || existing.payload_hash != leaf.payload_hash
                        || existing.activation_epoch != leaf.activation_epoch
                    {
                        return Err(ProcessPackageError::Invalid(format!(
                            "destination governance proposal {} already exists with different facts",
                            hex_hash(proposal_id)
                        )));
                    }
                    duplicate_remote = duplicate_remote.saturating_add(1);
                    continue;
                }

                let nonce = rpc
                    .account_next_index(submitter_account)
                    .await
                    .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
                let claim = ImportedGovernanceProposalClaim {
                    version: leaf.version,
                    leaf,
                    summary_hash: package.header.summary_hash,
                    package_hash: package.package_hash,
                };
                let extrinsic = build_import_governance_proposal_extrinsic(
                    submitter_pair,
                    submitter_account.clone(),
                    spec_version,
                    transaction_version,
                    genesis_hash,
                    nonce,
                    claim,
                )
                .map_err(|error| ProcessPackageError::Invalid(error.to_string()))?;
                let tx_hash = rpc
                    .submit_extrinsic(extrinsic)
                    .await
                    .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
                wait_for_governance_proposal(rpc, proposal_id)
                    .await
                    .map_err(ProcessPackageError::Retryable)?;
                tx_hashes.push(tx_hash);
                imported = imported.saturating_add(1);
            }
            GovernanceLeaf::AckV1(leaf) => {
                let proposal_id = leaf.proposal_id;
                let acknowledging_domain = leaf.acknowledging_domain;
                let proposal = rpc
                    .governance_proposal(proposal_id)
                    .await
                    .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
                let Some(proposal) = proposal else {
                    return Err(ProcessPackageError::Retryable(format!(
                        "governance ack {} from {} arrived before proposal facts were known locally",
                        hex_hash(proposal_id),
                        acknowledging_domain
                    )));
                };
                if proposal.source_domain != leaf.source_domain
                    || proposal.target_domains != leaf.target_domains
                    || proposal.payload_hash != leaf.payload_hash
                    || proposal.activation_epoch != leaf.activation_epoch
                {
                    return Err(ProcessPackageError::Invalid(format!(
                        "destination governance proposal {} does not match ack proof facts",
                        hex_hash(proposal_id)
                    )));
                }
                if rpc
                    .governance_ack_record(proposal_id, acknowledging_domain)
                    .await
                    .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?
                    .is_some()
                {
                    duplicate_remote = duplicate_remote.saturating_add(1);
                    continue;
                }

                let nonce = rpc
                    .account_next_index(submitter_account)
                    .await
                    .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
                let claim = ImportedGovernanceAckClaim {
                    version: leaf.version,
                    leaf,
                    summary_hash: package.header.summary_hash,
                    package_hash: package.package_hash,
                };
                let extrinsic = build_import_governance_ack_extrinsic(
                    submitter_pair,
                    submitter_account.clone(),
                    spec_version,
                    transaction_version,
                    genesis_hash,
                    nonce,
                    claim,
                )
                .map_err(|error| ProcessPackageError::Invalid(error.to_string()))?;
                let tx_hash = rpc
                    .submit_extrinsic(extrinsic)
                    .await
                    .map_err(|error| ProcessPackageError::Retryable(error.to_string()))?;
                wait_for_governance_ack(rpc, proposal_id, acknowledging_domain)
                    .await
                    .map_err(ProcessPackageError::Retryable)?;
                tx_hashes.push(tx_hash);
                imported = imported.saturating_add(1);
            }
        }
    }

    let final_state = if imported > 0 {
        ImporterPackageState::AckedVerified
    } else {
        ImporterPackageState::AckedDuplicateRemote
    };
    let reason = match final_state {
        ImporterPackageState::AckedVerified => {
            "governance proofs verified and imported on destination chain".into()
        }
        ImporterPackageState::AckedDuplicateRemote => {
            format!("all governance proofs were already known on destination chain ({duplicate_remote})")
        }
        _ => "governance package processing completed".into(),
    };

    Ok(ProcessPackageOutcome {
        final_state,
        tx_hashes,
        reason,
    })
}

async fn wait_for_observed_import_status(
    rpc: &NodeRpcClient,
    export_id: ialp_common_types::ExportId,
    expected: ialp_common_types::ImportObservationStatus,
) -> Result<(), String> {
    for _ in 0..60 {
        match rpc.observed_import(export_id).await {
            Ok(Some(record)) if record.status == expected => return Ok(()),
            Ok(_) => sleep(Duration::from_millis(PROCESSOR_TICK_MILLIS)).await,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(format!(
        "timed out waiting for observed import 0x{} to reach status {:?}",
        hex::encode(export_id),
        expected
    ))
}

async fn wait_for_export_record_status(
    rpc: &NodeRpcClient,
    export_id: ialp_common_types::ExportId,
    expected: ialp_common_types::ExportStatus,
) -> Result<(), String> {
    for _ in 0..60 {
        match rpc.export_record(export_id).await {
            Ok(Some(record)) if record.status == expected => return Ok(()),
            Ok(_) => sleep(Duration::from_millis(PROCESSOR_TICK_MILLIS)).await,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(format!(
        "timed out waiting for export record 0x{} to reach status {:?}",
        hex::encode(export_id),
        expected
    ))
}

async fn wait_for_governance_proposal(
    rpc: &NodeRpcClient,
    proposal_id: [u8; 32],
) -> Result<(), String> {
    for _ in 0..60 {
        match rpc.governance_proposal(proposal_id).await {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => sleep(Duration::from_millis(PROCESSOR_TICK_MILLIS)).await,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(format!(
        "timed out waiting for governance proposal {} to be recorded",
        hex_hash(proposal_id)
    ))
}

async fn wait_for_governance_ack(
    rpc: &NodeRpcClient,
    proposal_id: [u8; 32],
    acknowledging_domain: DomainId,
) -> Result<(), String> {
    for _ in 0..60 {
        match rpc
            .governance_ack_record(proposal_id, acknowledging_domain)
            .await
        {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => sleep(Duration::from_millis(PROCESSOR_TICK_MILLIS)).await,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(format!(
        "timed out waiting for governance ack {} from {} to be recorded",
        hex_hash(proposal_id),
        acknowledging_domain
    ))
}

fn inspect_package_for_ingest(
    package: &CertifiedSummaryPackage,
    importer_domain: DomainId,
) -> anyhow::Result<PackageIngestView> {
    if package.inclusion_proofs.len() < 2 {
        bail!("transport packages require at least one non-summary inclusion proof");
    }

    let mut target_domain = None;
    let mut item_count = 0u32;
    for (index, encoded) in package.inclusion_proofs.iter().enumerate() {
        let proof = InclusionProof::decode(&mut &encoded[..])
            .map_err(|error| anyhow!("failed to decode package inclusion proof: {error}"))?;
        match (index, proof) {
            (0, InclusionProof::SummaryHeaderStorageV1(_)) => {}
            (
                0,
                InclusionProof::ExportV1(_)
                | InclusionProof::FinalizedImportV1(_)
                | InclusionProof::GovernanceV1(_),
            ) => {
                bail!("summary-header storage proof must remain at inclusion_proofs[0]")
            }
            (_, InclusionProof::SummaryHeaderStorageV1(_)) => {
                bail!("summary-header storage proof may only appear at inclusion_proofs[0]")
            }
            (_, InclusionProof::ExportV1(proof)) => {
                item_count = item_count.saturating_add(1);
                match target_domain {
                    Some(existing) if existing != proof.leaf.target_domain => {
                        bail!("package contains mixed target domains")
                    }
                    None => target_domain = Some(proof.leaf.target_domain),
                    _ => {}
                }
                if proof.leaf.source_domain != package.header.domain_id
                        || proof.leaf.source_epoch_id != package.header.epoch_id
                {
                    bail!("package export proofs do not match source domain or epoch");
                }
            }
            (_, InclusionProof::FinalizedImportV1(proof)) => {
                item_count = item_count.saturating_add(1);
                match target_domain {
                    Some(existing) if existing != proof.leaf.source_domain => {
                        bail!("completion package contains mixed source domains")
                    }
                    None => target_domain = Some(proof.leaf.source_domain),
                    _ => {}
                }
                if proof.leaf.target_domain != package.header.domain_id {
                    bail!("completion proof leaf target domain does not match package source domain");
                }
            }
            (_, InclusionProof::GovernanceV1(proof)) => {
                item_count = item_count.saturating_add(1);
                match target_domain {
                    Some(existing) if existing != proof.leaf.target_domain() => {
                        bail!("governance package contains mixed target domains")
                    }
                    None => target_domain = Some(proof.leaf.target_domain()),
                    _ => {}
                }
                match &proof.leaf {
                    GovernanceLeaf::ProposalV1(leaf) => {
                        if leaf.source_domain != package.header.domain_id
                            || leaf.approval_epoch != package.header.epoch_id
                        {
                            bail!("package governance proposal proofs do not match source domain or epoch");
                        }
                    }
                    GovernanceLeaf::AckV1(leaf) => {
                        if leaf.acknowledged_epoch != package.header.epoch_id {
                            bail!("package governance ack proofs do not match source epoch");
                        }
                    }
                }
            }
        }
    }

    let target_domain =
        target_domain.ok_or_else(|| anyhow!("package has no supported inclusion proofs"))?;
    if target_domain != importer_domain {
        bail!(
            "package target domain {} does not match importer domain {}",
            target_domain,
            importer_domain
        );
    }

    Ok(PackageIngestView {
        source_domain: package.header.domain_id,
        target_domain,
        epoch_id: package.header.epoch_id,
        export_count: item_count,
    })
}

fn package_status_view(record: &PackageRecord) -> ImporterPackageStatusView {
    ImporterPackageStatusView {
        source_domain: record.source_domain,
        target_domain: record.target_domain,
        epoch_id: record.epoch_id,
        package_hash: decode_hex_hash(&record.package_hash).unwrap_or([0u8; 32]),
        summary_hash: decode_hex_hash(&record.summary_hash).unwrap_or([0u8; 32]),
        state: record.state,
        terminal: record.state.is_terminal(),
        reason: record.reason.clone(),
        export_count: record.export_count,
        tx_hashes: record.tx_hashes.clone(),
    }
}

fn verify_package_hash(package: &CertifiedSummaryPackage) -> anyhow::Result<()> {
    let expected = package.compute_package_hash();
    if package.package_hash != expected {
        bail!(
            "package hash mismatch: expected {}, found {}",
            hex_hash(expected),
            hex_hash(package.package_hash)
        );
    }
    Ok(())
}

fn verify_summary_chain_context(
    package: &CertifiedSummaryPackage,
) -> anyhow::Result<ialp_common_types::GrandpaFinalityCertificate> {
    let certificate = match &package.certificate {
        SummaryCertificate::GrandpaV1(certificate) => certificate.clone(),
    };

    let summary_proof = decode_summary_storage_proof(
        package
            .inclusion_proofs
            .first()
            .ok_or_else(|| anyhow!("package is missing inclusion_proofs[0]"))?,
    )?;
    if summary_proof.proof_block_number != certificate.proof_block_number
        || summary_proof.proof_block_hash != certificate.proof_block_hash
    {
        bail!("summary-header storage proof targets a different proof block than the certificate");
    }

    let proof_block_header = ialp_runtime::Header::decode(
        &mut &summary_proof.proof_block_header[..],
    )
    .map_err(|error| anyhow!("failed to decode proof block header from summary proof: {error}"))?;
    if proof_block_header.hash() != H256::from(summary_proof.proof_block_hash) {
        bail!("proof block header hash does not match certificate proof block hash");
    }
    if *proof_block_header.number() != summary_proof.proof_block_number {
        bail!("proof block header number does not match certificate proof block number");
    }
    verify_summary_storage_proof(package, &summary_proof, &proof_block_header)?;
    verify_ancestry_chain(&certificate)?;
    Ok(certificate)
}

fn verify_summary_storage_proof(
    package: &CertifiedSummaryPackage,
    summary_proof: &ialp_common_types::SummaryHeaderStorageProof,
    proof_block_header: &ialp_runtime::Header,
) -> anyhow::Result<()> {
    let expected_key = summary_header_storage_key(package.header.epoch_id);
    if summary_proof.storage_key != expected_key {
        bail!("summary-header storage proof does not target SummaryHeaders[epoch_id]");
    }

    let verified_values = read_proof_check::<BlakeTwo256, _>(
        *proof_block_header.state_root(),
        StorageProof::new(summary_proof.trie_nodes.clone()),
        [summary_proof.storage_key.as_slice()],
    )
    .map_err(|error| anyhow!("summary-header storage proof verification failed: {error}"))?;

    match verified_values.get(&summary_proof.storage_key) {
        Some(Some(value)) if value == &package.header.encode() => Ok(()),
        _ => bail!("summary-header storage proof did not return exact SCALE(package.header) bytes"),
    }
}

fn verify_ancestry_chain(
    certificate: &ialp_common_types::GrandpaFinalityCertificate,
) -> anyhow::Result<()> {
    let mut current_hash = H256::from(certificate.target_block_hash);
    let mut current_number = certificate.target_block_number;

    for encoded in &certificate.ancestry_headers {
        let header = ialp_runtime::Header::decode(&mut &encoded[..])
            .map_err(|error| anyhow!("failed to decode ancestry header: {error}"))?;
        if header.parent_hash() != &current_hash {
            bail!("ancestry header chain does not descend from the staged target block");
        }
        let header_number = *header.number();
        if header_number != current_number + 1 {
            bail!("ancestry header numbering is not contiguous");
        }
        current_hash = header.hash();
        current_number = header_number;
    }

    if current_hash != H256::from(certificate.proof_block_hash)
        || current_number != certificate.proof_block_number
    {
        bail!("ancestry headers do not terminate at the certificate proof block");
    }

    Ok(())
}

fn verify_grandpa_certificate(
    certificate: &ialp_common_types::GrandpaFinalityCertificate,
    source_domain: DomainId,
) -> anyhow::Result<()> {
    let source_config = load_domain_config(source_domain, None)
        .with_context(|| format!("failed to load source domain config for {source_domain}"))?;
    let authorities = source_config
        .config
        .authorities
        .iter()
        .map(|authority| Ok((get_from_seed::<GrandpaId>(&authority.grandpa_seed)?, 1u64)))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let justification = sp_consensus_grandpa::GrandpaJustification::<ialp_runtime::Header>::decode(
        &mut &certificate.justification[..],
    )
    .map_err(|error| anyhow!("failed to decode GRANDPA justification: {error}"))?;

    if justification.commit.target_hash != H256::from(certificate.proof_block_hash)
        || justification.commit.target_number != certificate.proof_block_number
    {
        bail!("GRANDPA justification target does not match certificate proof block");
    }

    let wrapper: GrandpaJustification<ialp_runtime::Block> = justification.into();
    wrapper
        .verify(certificate.grandpa_set_id, &authorities)
        .map_err(|error| anyhow!("GRANDPA justification verification failed: {error}"))?;
    Ok(())
}

fn verify_package_flow(
    package: &CertifiedSummaryPackage,
    importer_domain: DomainId,
) -> anyhow::Result<VerifiedPackageItems> {
    if package.inclusion_proofs.len() < 2 {
        bail!("package must include at least one non-summary inclusion proof");
    }

    Ok(VerifiedPackageItems {
        export_proofs: verify_forward_export_proofs(package, importer_domain)?,
        finalized_import_proofs: verify_finalized_import_proofs(package, importer_domain)?,
        governance_proofs: verify_governance_proofs(package, importer_domain)?,
    })
}

fn verify_forward_export_proofs(
    package: &CertifiedSummaryPackage,
    importer_domain: DomainId,
) -> anyhow::Result<Vec<ialp_common_types::ExportInclusionProof>> {
    let mut export_proofs = Vec::new();
    for encoded in &package.inclusion_proofs[1..] {
        let proof = match InclusionProof::decode(&mut &encoded[..])
            .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?
        {
            InclusionProof::ExportV1(proof) => proof,
            InclusionProof::SummaryHeaderStorageV1(_) => {
                bail!("summary-header proof may appear only at inclusion_proofs[0]")
            }
            InclusionProof::FinalizedImportV1(_) => {
                continue;
            }
            InclusionProof::GovernanceV1(_) => {
                continue;
            }
        };

        if proof.leaf.export_hash != proof.leaf.compute_export_hash() {
            bail!("export proof leaf hash does not match canonical export leaf contents");
        }
        if proof.leaf.source_domain != package.header.domain_id
            || proof.leaf.source_epoch_id != package.header.epoch_id
        {
            bail!("export proof leaf does not match package source domain or epoch");
        }
        if proof.leaf.target_domain != importer_domain {
            bail!(
                "package target domain {} does not match importer domain {}",
                proof.leaf.target_domain,
                importer_domain
            );
        }
        if !ialp_common_types::verify_export_inclusion_proof(package.header.export_root, &proof) {
            bail!("export inclusion proof failed against header.export_root");
        }

        export_proofs.push(proof);
    }

    Ok(export_proofs)
}

fn verify_finalized_import_proofs(
    package: &CertifiedSummaryPackage,
    importer_domain: DomainId,
) -> anyhow::Result<Vec<FinalizedImportInclusionProof>> {
    let mut proofs = Vec::new();
    for encoded in &package.inclusion_proofs[1..] {
        let proof = match InclusionProof::decode(&mut &encoded[..])
            .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?
        {
            InclusionProof::FinalizedImportV1(proof) => proof,
            InclusionProof::SummaryHeaderStorageV1(_) => {
                bail!("summary-header proof may appear only at inclusion_proofs[0]")
            }
            InclusionProof::ExportV1(_) => {
                continue;
            }
            InclusionProof::GovernanceV1(_) => {
                continue;
            }
        };

        if proof.leaf.import_hash != proof.leaf.compute_import_hash() {
            bail!("finalized-import proof leaf hash does not match canonical finalized import leaf contents");
        }
        if proof.leaf.target_domain != package.header.domain_id {
            bail!("finalized-import proof leaf target domain does not match package source domain");
        }
        if proof.leaf.source_domain != importer_domain {
            bail!(
                "completion package target domain {} does not match importer domain {}",
                proof.leaf.source_domain,
                importer_domain
            );
        }
        if !ialp_common_types::verify_finalized_import_inclusion_proof(package.header.import_root, &proof)
        {
            bail!("finalized-import inclusion proof failed against header.import_root");
        }

        proofs.push(proof);
    }

    Ok(proofs)
}

fn verify_governance_proofs(
    package: &CertifiedSummaryPackage,
    importer_domain: DomainId,
) -> anyhow::Result<Vec<GovernanceInclusionProof>> {
    let mut proofs = Vec::new();
    for encoded in &package.inclusion_proofs[1..] {
        let proof = match InclusionProof::decode(&mut &encoded[..])
            .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?
        {
            InclusionProof::GovernanceV1(proof) => proof,
            InclusionProof::SummaryHeaderStorageV1(_) => {
                bail!("summary-header proof may appear only at inclusion_proofs[0]")
            }
            InclusionProof::ExportV1(_) | InclusionProof::FinalizedImportV1(_) => {
                continue;
            }
        };

        if !ialp_common_types::verify_governance_inclusion_proof(package.header.governance_root, &proof)
        {
            bail!("governance inclusion proof failed against header.governance_root");
        }
        match &proof.leaf {
            GovernanceLeaf::ProposalV1(leaf) => {
                if leaf.leaf_hash != leaf.compute_leaf_hash() {
                    bail!("governance proposal proof leaf hash does not match canonical contents");
                }
                if leaf.target_domain != importer_domain {
                    bail!(
                        "governance proposal target domain {} does not match importer domain {}",
                        leaf.target_domain,
                        importer_domain
                    );
                }
                if leaf.source_domain != package.header.domain_id
                    || leaf.approval_epoch != package.header.epoch_id
                {
                    bail!("governance proposal proof does not match package source domain/epoch");
                }
            }
            GovernanceLeaf::AckV1(leaf) => {
                if leaf.leaf_hash != leaf.compute_leaf_hash() {
                    bail!("governance ack proof leaf hash does not match canonical contents");
                }
                if leaf.target_domain != importer_domain {
                    bail!(
                        "governance ack target domain {} does not match importer domain {}",
                        leaf.target_domain,
                        importer_domain
                    );
                }
                if leaf.acknowledged_epoch != package.header.epoch_id {
                    bail!("governance ack proof does not match package source epoch");
                }
            }
        }

        proofs.push(proof);
    }

    Ok(proofs)
}

fn decode_summary_storage_proof(
    bytes: &[u8],
) -> anyhow::Result<ialp_common_types::SummaryHeaderStorageProof> {
    match InclusionProof::decode(&mut &bytes[..])
        .map_err(|error| anyhow!("failed to decode summary proof: {error}"))?
    {
        InclusionProof::SummaryHeaderStorageV1(proof) => Ok(proof),
        InclusionProof::ExportV1(_)
        | InclusionProof::FinalizedImportV1(_)
        | InclusionProof::GovernanceV1(_) => {
            bail!("expected summary-header storage proof at index 0")
        }
    }
}

fn submitter_account_id(pair: &sr25519::Pair) -> ialp_runtime::AccountId {
    <ialp_runtime::Signature as Verify>::Signer::from(pair.public()).into_account()
}

fn ensure_runtime_version_matches(
    runtime_version: &rpc_client::RuntimeVersionView,
) -> anyhow::Result<()> {
    if runtime_version.spec_version != ialp_runtime::VERSION.spec_version
        || runtime_version.transaction_version != ialp_runtime::VERSION.transaction_version
    {
        bail!(
            "remote runtime version {} / {} does not match local importer runtime {} / {}",
            runtime_version.spec_version,
            runtime_version.transaction_version,
            ialp_runtime::VERSION.spec_version,
            ialp_runtime::VERSION.transaction_version
        );
    }
    Ok(())
}

fn build_observed_import_extrinsic(
    submitter_pair: &sr25519::Pair,
    account_id: ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    nonce: u32,
    claim: ObservedImportClaim,
) -> anyhow::Result<Vec<u8>> {
    let call =
        ialp_runtime::RuntimeCall::Transfers(TransfersCall::observe_verified_import { claim });
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

fn build_finalize_verified_import_extrinsic(
    submitter_pair: &sr25519::Pair,
    account_id: ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    nonce: u32,
    export_id: ialp_common_types::ExportId,
) -> anyhow::Result<Vec<u8>> {
    let call =
        ialp_runtime::RuntimeCall::Transfers(TransfersCall::finalize_verified_import { export_id });
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

fn build_acknowledge_remote_finalization_extrinsic(
    submitter_pair: &sr25519::Pair,
    account_id: ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    nonce: u32,
    claim: RemoteFinalizationClaim,
) -> anyhow::Result<Vec<u8>> {
    let call = ialp_runtime::RuntimeCall::Transfers(
        TransfersCall::acknowledge_remote_finalization { claim },
    );
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

fn build_import_governance_proposal_extrinsic(
    submitter_pair: &sr25519::Pair,
    account_id: ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    nonce: u32,
    claim: ImportedGovernanceProposalClaim,
) -> anyhow::Result<Vec<u8>> {
    let call = ialp_runtime::RuntimeCall::Governance(
        GovernanceCall::import_verified_governance_proposal { claim },
    );
    build_signed_extrinsic(
        submitter_pair,
        account_id,
        spec_version,
        transaction_version,
        genesis_hash,
        nonce,
        call,
    )
}

fn build_import_governance_ack_extrinsic(
    submitter_pair: &sr25519::Pair,
    account_id: ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    nonce: u32,
    claim: ImportedGovernanceAckClaim,
) -> anyhow::Result<Vec<u8>> {
    let call = ialp_runtime::RuntimeCall::Governance(
        GovernanceCall::import_verified_governance_ack { claim },
    );
    build_signed_extrinsic(
        submitter_pair,
        account_id,
        spec_version,
        transaction_version,
        genesis_hash,
        nonce,
        call,
    )
}

fn build_signed_extrinsic(
    submitter_pair: &sr25519::Pair,
    account_id: ialp_runtime::AccountId,
    spec_version: u32,
    transaction_version: u32,
    genesis_hash: H256,
    nonce: u32,
    call: ialp_runtime::RuntimeCall,
) -> anyhow::Result<Vec<u8>> {
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

fn hex_hash(hash: [u8; 32]) -> String {
    format!("0x{}", hex::encode(hash))
}

fn get_from_seed<TPublic: Public>(seed: &str) -> anyhow::Result<<TPublic::Pair as Pair>::Public>
where
    TPublic::Pair: Pair,
{
    TPublic::Pair::from_string(seed, None)
        .map(|pair| pair.public())
        .map_err(|error| anyhow!("failed to derive public key from seed '{seed}': {error}"))
}

fn render_json(value: serde_json::Value, _as_json: bool) {
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("json output should serialize")
    );
}

fn unix_now_millis() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is set before unix epoch")?
        .as_millis()
        .try_into()
        .context("unix timestamp does not fit into u64")
}

impl ImporterSettings {
    fn load(
        domain: DomainId,
        node_url: Option<String>,
        store_dir: Option<PathBuf>,
        transport_config: Option<PathBuf>,
        submitter_suri: Option<String>,
    ) -> anyhow::Result<Self> {
        let loaded = load_domain_config(domain, None)
            .with_context(|| format!("failed to load importer config for domain {domain}"))?;
        let node_url = node_url
            .unwrap_or_else(|| format!("ws://127.0.0.1:{}", loaded.config.network.rpc_port));
        let store_dir =
            store_dir.unwrap_or_else(|| PathBuf::from("var/importer").join(domain.as_str()));
        let store = Arc::new(Store::new(store_dir, domain)?);
        let listen_addr = transport_config
            .map(|path| load_transport_config(Some(&path)))
            .transpose()?
            .map(|loaded| {
                loaded
                    .config
                    .importers
                    .get(&domain)
                    .map(|config| config.listen_addr.clone())
                    .ok_or_else(|| {
                        anyhow!("transport config is missing importer endpoint for {domain}")
                    })
            })
            .transpose()?;
        Ok(Self {
            domain,
            node_url,
            listen_addr,
            store,
            submitter_suri,
        })
    }
}

struct PackageIngestView {
    source_domain: DomainId,
    target_domain: DomainId,
    epoch_id: u64,
    export_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ialp_common_types::{
        build_governance_inclusion_proof, governance_proposal_id, GovernancePayload,
        GovernanceProposalLeaf, GovernanceProposalLeafHashInput, GrandpaFinalityCertificate,
        SummaryCertificationBundle, SummaryHeaderStorageProof, EpochSummaryHeader,
        GOVERNANCE_PROPOSAL_LEAF_VERSION, SUMMARY_HEADER_STORAGE_PROOF_VERSION,
    };

    fn sample_governance_package() -> CertifiedSummaryPackage {
        let header = EpochSummaryHeader {
            version: 1,
            domain_id: DomainId::Earth,
            epoch_id: 4,
            prev_summary_hash: [0u8; 32],
            start_block_height: 9,
            end_block_height: 12,
            state_root: [1u8; 32],
            block_root: [2u8; 32],
            tx_root: [3u8; 32],
            event_root: [4u8; 32],
            export_root: [5u8; 32],
            import_root: [6u8; 32],
            governance_root: [7u8; 32],
            validator_set_hash: [8u8; 32],
            summary_hash: [9u8; 32],
        };
        let leaf = GovernanceLeaf::ProposalV1(GovernanceProposalLeaf::from_hash_input(
            GovernanceProposalLeafHashInput {
                version: GOVERNANCE_PROPOSAL_LEAF_VERSION,
                proposal_id: governance_proposal_id(DomainId::Earth, 0),
                source_domain: DomainId::Earth,
                target_domain: DomainId::Moon,
                target_domains: vec![DomainId::Moon],
                proposer: [10u8; 32],
                payload_hash: GovernancePayload::SetProtocolVersion { new_version: 2 }
                    .payload_hash(),
                new_protocol_version: 2,
                created_epoch: 3,
                voting_start_epoch: 3,
                voting_end_epoch: 4,
                approval_epoch: 4,
                activation_epoch: 8,
            },
        ));
        let bundle = SummaryCertificationBundle {
            certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                version: 1,
                grandpa_set_id: 0,
                target_block_number: 13,
                target_block_hash: [10u8; 32],
                proof_block_number: 13,
                proof_block_hash: [10u8; 32],
                justification: vec![1, 2, 3],
                ancestry_headers: Vec::new(),
            }),
            summary_header_storage_proof: SummaryHeaderStorageProof {
                version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                proof_block_number: 13,
                proof_block_hash: [10u8; 32],
                proof_block_header: Vec::new(),
                storage_key: summary_header_storage_key(4),
                trie_nodes: Vec::new(),
            },
        };
        let mut package = CertifiedSummaryPackage::from_bundle_with_governance_proofs(
            header,
            bundle,
            vec![build_governance_inclusion_proof(core::slice::from_ref(&leaf), leaf.leaf_hash())
                .expect("governance proof")],
        );
        package.header.governance_root = ialp_common_types::governance_merkle_root(
            DomainId::Earth,
            4,
            9,
            12,
            core::slice::from_ref(&leaf),
        );
        package.package_hash = package.compute_package_hash();
        package
    }

    #[test]
    fn governance_package_is_accepted_for_matching_importer_domain() {
        let package = sample_governance_package();
        let view = inspect_package_for_ingest(&package, DomainId::Moon).expect("package ingest");

        assert_eq!(view.source_domain, DomainId::Earth);
        assert_eq!(view.target_domain, DomainId::Moon);
        assert_eq!(view.export_count, 1);
        let verified = verify_package_flow(&package, DomainId::Moon).expect("verified");
        assert_eq!(verified.governance_proofs.len(), 1);
    }
}
