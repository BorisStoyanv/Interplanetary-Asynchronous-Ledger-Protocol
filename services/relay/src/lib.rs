mod cli;
mod store;

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context};
use axum::{
    body::Bytes, extract::State, http::StatusCode, response::IntoResponse, routing::post, Json,
    Router,
};
use chrono::{DateTime, Utc};
use clap::Parser;
use codec::Decode;
use ialp_common_config::{load_transport_config, LinkProfileConfig, TransportConfig};
use ialp_common_types::{
    CertifiedSummaryPackage, ImporterPackageStatusView, InclusionProof, RelayPackageEnvelopeV1,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, sync::Mutex, time::sleep};

use crate::{
    cli::{Cli, Commands},
    store::{decode_hex_hash, RelayIndex, RelayPackageRecord, RelayQueueState, Store},
};

#[derive(Clone)]
struct AppState {
    settings: RelaySettings,
    store: Arc<Store>,
    index: Arc<Mutex<RelayIndex>>,
    client: Client,
}

#[derive(Clone)]
struct RelaySettings {
    listen_addr: String,
    store_dir: PathBuf,
    scheduler_tick_millis: u64,
    ack_poll_millis: u64,
    transport: TransportConfig,
}

#[derive(Debug, Serialize)]
struct RelaySubmitReceipt {
    accepted: bool,
    idempotent: bool,
    state: String,
}

#[derive(Debug, Deserialize)]
struct ImporterIngestReceipt {
    accepted: bool,
    idempotent: bool,
    status: ImporterPackageStatusView,
}

pub async fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => run_relay(args).await,
        Commands::Status(args) => show_status(args),
        Commands::Show(args) => show_entry(args),
    }
}

async fn run_relay(args: cli::RunArgs) -> anyhow::Result<()> {
    let settings = RelaySettings::load(args.transport_config)?;
    let store = Arc::new(Store::new(settings.store_dir.clone())?);
    let mut index = store.load_index()?;
    normalize_restart_state(&mut index, settings.ack_poll_millis, unix_now_millis()?)?;
    store.save_index(&index)?;

    let app_state = Arc::new(AppState {
        settings: settings.clone(),
        store,
        index: Arc::new(Mutex::new(index)),
        client: Client::new(),
    });

    let scheduler_state = app_state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = scheduler_tick(&scheduler_state).await {
                eprintln!("relay scheduler tick failed: {error:#}");
            }
            sleep(Duration::from_millis(
                scheduler_state.settings.scheduler_tick_millis,
            ))
            .await;
        }
    });

    let router = Router::new()
        .route("/api/v1/packages", post(submit_package))
        .with_state(app_state.clone());

    let listener = TcpListener::bind(&settings.listen_addr)
        .await
        .with_context(|| format!("failed to bind relay listener {}", settings.listen_addr))?;
    axum::serve(listener, router)
        .await
        .context("relay HTTP server exited unexpectedly")
}

fn show_status(args: cli::StatusArgs) -> anyhow::Result<()> {
    let settings = RelaySettings::load(args.transport_config)?;
    let store = Store::new(settings.store_dir.clone())?;
    let index = store.load_index()?;
    let state_filter = args.state.as_deref().map(str::to_owned);
    let records = index
        .packages
        .iter()
        .filter(|record| {
            args.source_domain
                .map(|source| source == record.source_domain)
                .unwrap_or(true)
                && args
                    .target_domain
                    .map(|target| target == record.target_domain)
                    .unwrap_or(true)
                && state_filter
                    .as_ref()
                    .map(|state| state == relay_state_name(&record.state))
                    .unwrap_or(true)
        })
        .map(RelayPackageRecord::json_summary)
        .collect::<Vec<_>>();

    render_json(
        serde_json::json!({
            "listen_addr": settings.listen_addr,
            "store_dir": settings.store_dir,
            "packages": records,
        }),
        args.json,
    );
    Ok(())
}

fn show_entry(args: cli::ShowArgs) -> anyhow::Result<()> {
    let settings = RelaySettings::load(args.transport_config)?;
    let store = Store::new(settings.store_dir.clone())?;
    let index = store.load_index()?;
    let package_hash = decode_hex_hash(&args.package_hash)?;
    let record = index
        .record(
            args.source_domain,
            args.target_domain,
            args.epoch,
            package_hash,
        )
        .ok_or_else(|| anyhow!("relay package entry not found"))?;
    render_json(record.json_summary(), args.json);
    Ok(())
}

async fn submit_package(State(app): State<Arc<AppState>>, body: Bytes) -> impl IntoResponse {
    let response = async {
        let (envelope, _package) = decode_and_validate_envelope(&body)?;
        let now = unix_now_millis()?;
        let mut index = app.index.lock().await;
        let (record, idempotent) = app.store.accept_envelope(&mut index, &envelope, now)?;
        app.store.save_index(&index)?;

        let status = if idempotent {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        let receipt = RelaySubmitReceipt {
            accepted: true,
            idempotent,
            state: relay_state_name(&record.state).to_string(),
        };
        Ok::<_, anyhow::Error>((status, Json(receipt)))
    }
    .await;

    match response {
        Ok(success) => success.into_response(),
        Err(error) => {
            let status = if error.to_string().contains("different payload bytes") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, error.to_string()).into_response()
        }
    }
}

async fn scheduler_tick(app: &Arc<AppState>) -> anyhow::Result<()> {
    let work = {
        let index = app.index.lock().await;
        index
            .packages
            .iter()
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

    for (source_domain, target_domain, epoch_id, package_hash_hex) in work {
        let package_hash = decode_hex_hash(&package_hash_hex)?;
        let record = {
            let index = app.index.lock().await;
            index
                .record(source_domain, target_domain, epoch_id, package_hash)
                .cloned()
        };
        let Some(record) = record else {
            continue;
        };

        match record.state {
            RelayQueueState::Queued => {
                schedule_record(app, record).await?;
            }
            RelayQueueState::Scheduled
            | RelayQueueState::BlockedByBlackout
            | RelayQueueState::Retrying => {
                if due_now(record.next_delivery_at_unix_ms, unix_now_millis()?) {
                    attempt_delivery(app, record).await?;
                }
            }
            RelayQueueState::Delivered => {
                if due_now(record.next_ack_poll_at_unix_ms, unix_now_millis()?) {
                    poll_importer_status(app, record).await?;
                }
            }
            RelayQueueState::InDelivery
            | RelayQueueState::ImporterAcked
            | RelayQueueState::Failed => {}
        }
    }

    Ok(())
}

async fn schedule_record(app: &Arc<AppState>, record: RelayPackageRecord) -> anyhow::Result<()> {
    let link = app
        .settings
        .transport
        .link(record.source_domain, record.target_domain)
        .ok_or_else(|| anyhow!("missing link profile"))?;
    let due = record
        .relay_accepted_at_unix_ms
        .saturating_add(link.base_one_way_delay_seconds.saturating_mul(1_000));
    let scheduled_at = next_delivery_after(link, due)?;
    let next_state = if scheduled_at == due {
        RelayQueueState::Scheduled
    } else {
        RelayQueueState::BlockedByBlackout
    };
    update_record(app, &record, move |current| {
        current.state = next_state;
        current.next_delivery_at_unix_ms = Some(scheduled_at);
        current.last_delivery_error = None;
    })
    .await
}

async fn attempt_delivery(app: &Arc<AppState>, record: RelayPackageRecord) -> anyhow::Result<()> {
    let now = unix_now_millis()?;
    let link = app
        .settings
        .transport
        .link(record.source_domain, record.target_domain)
        .ok_or_else(|| anyhow!("missing link profile"))?;
    if let Some(next_delivery_at) = record.next_delivery_at_unix_ms {
        let blocked_until = next_delivery_after(link, next_delivery_at)?;
        if blocked_until > now {
            return update_record(app, &record, move |current| {
                current.state = RelayQueueState::BlockedByBlackout;
                current.next_delivery_at_unix_ms = Some(blocked_until);
            })
            .await;
        }
    }

    update_record(app, &record, move |current| {
        current.state = RelayQueueState::InDelivery;
        current.delivery_attempts = current.delivery_attempts.saturating_add(1);
        current.last_delivery_error = None;
    })
    .await?;

    let payload = app.store.load_payload(
        record.source_domain,
        record.target_domain,
        record.epoch_id,
        decode_hex_hash(&record.package_hash)?,
    )?;
    let importer_url = importer_base_url(&app.settings.transport, record.target_domain)?;
    let response = app
        .client
        .post(format!("{}/api/v1/packages", importer_url))
        .header("content-type", "application/octet-stream")
        .body(payload)
        .send()
        .await;

    match response {
        Ok(response)
            if response.status() == StatusCode::OK || response.status() == StatusCode::ACCEPTED =>
        {
            let status: ImporterIngestReceipt = response
                .json()
                .await
                .context("failed to decode importer ingest receipt")?;
            let _ = (status.accepted, status.idempotent);
            if status.status.state.is_terminal() {
                let importer_state = status.status.state;
                let importer_reason = status.status.reason.clone();
                return update_record(app, &record, move |current| {
                    current.state = RelayQueueState::ImporterAcked;
                    current.delivered_at_unix_ms.get_or_insert(now);
                    current.completed_at_unix_ms = Some(now);
                    current.importer_state = Some(importer_state);
                    current.importer_reason = importer_reason;
                    current.last_importer_status_at_unix_ms = Some(now);
                    current.next_ack_poll_at_unix_ms = None;
                })
                .await;
            }

            update_record(app, &record, move |current| {
                current.state = RelayQueueState::Delivered;
                current.delivered_at_unix_ms.get_or_insert(now);
                current.importer_state = Some(status.status.state);
                current.importer_reason = status.status.reason;
                current.last_importer_status_at_unix_ms = Some(now);
                current.next_ack_poll_at_unix_ms =
                    Some(now.saturating_add(app.settings.ack_poll_millis));
                current.next_delivery_at_unix_ms = None;
            })
            .await
        }
        Ok(response) if response.status().is_client_error() => {
            let status = response.status();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "<non-utf8 importer error>".into());
            update_record(app, &record, move |current| {
                current.state = RelayQueueState::Failed;
                current.last_delivery_error =
                    Some(format!("importer returned {}: {}", status, message));
                current.completed_at_unix_ms = Some(now);
                current.next_delivery_at_unix_ms = None;
                current.next_ack_poll_at_unix_ms = None;
            })
            .await
        }
        Ok(response) => {
            schedule_retry(
                app,
                &record,
                now,
                link,
                format!("importer returned transient status {}", response.status()),
            )
            .await
        }
        Err(error) => {
            schedule_retry(
                app,
                &record,
                now,
                link,
                format!("importer delivery request failed: {error}"),
            )
            .await
        }
    }
}

async fn poll_importer_status(
    app: &Arc<AppState>,
    record: RelayPackageRecord,
) -> anyhow::Result<()> {
    let now = unix_now_millis()?;
    let importer_url = importer_base_url(&app.settings.transport, record.target_domain)?;
    let response = app
        .client
        .get(format!(
            "{}/api/v1/packages/{}/{}/{}/{}",
            importer_url,
            record.source_domain.as_str(),
            record.target_domain.as_str(),
            record.epoch_id,
            record.package_hash
        ))
        .send()
        .await;

    match response {
        Ok(response) if response.status().is_success() => {
            let status: ImporterPackageStatusView = response
                .json()
                .await
                .context("failed to decode importer package status")?;
            if status.state.is_terminal() {
                let importer_state = status.state;
                let importer_reason = status.reason.clone();
                return update_record(app, &record, move |current| {
                    current.state = RelayQueueState::ImporterAcked;
                    current.importer_state = Some(importer_state);
                    current.importer_reason = importer_reason;
                    current.last_importer_status_at_unix_ms = Some(now);
                    current.completed_at_unix_ms = Some(now);
                    current.next_ack_poll_at_unix_ms = None;
                })
                .await;
            }

            update_record(app, &record, move |current| {
                current.importer_state = Some(status.state);
                current.importer_reason = status.reason;
                current.last_importer_status_at_unix_ms = Some(now);
                current.next_ack_poll_at_unix_ms =
                    Some(now.saturating_add(app.settings.ack_poll_millis));
            })
            .await
        }
        Ok(response) if response.status().is_client_error() => {
            let status = response.status();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "<non-utf8 importer status error>".into());
            update_record(app, &record, move |current| {
                current.state = RelayQueueState::Failed;
                current.last_delivery_error =
                    Some(format!("importer status returned {}: {}", status, message));
                current.completed_at_unix_ms = Some(now);
                current.next_ack_poll_at_unix_ms = None;
            })
            .await
        }
        Ok(response) => {
            update_record(app, &record, move |current| {
                current.last_delivery_error = Some(format!(
                    "importer status polling returned {}",
                    response.status()
                ));
                current.next_ack_poll_at_unix_ms =
                    Some(now.saturating_add(app.settings.ack_poll_millis));
            })
            .await
        }
        Err(error) => {
            update_record(app, &record, move |current| {
                current.last_delivery_error =
                    Some(format!("importer status polling failed: {error}"));
                current.next_ack_poll_at_unix_ms =
                    Some(now.saturating_add(app.settings.ack_poll_millis));
            })
            .await
        }
    }
}

async fn schedule_retry(
    app: &Arc<AppState>,
    record: &RelayPackageRecord,
    now: u64,
    link: &LinkProfileConfig,
    error: String,
) -> anyhow::Result<()> {
    if link.max_attempts != 0 && record.delivery_attempts >= link.max_attempts {
        return update_record(app, record, move |current| {
            current.state = RelayQueueState::Failed;
            current.last_delivery_error = Some(error);
            current.completed_at_unix_ms = Some(now);
            current.next_delivery_at_unix_ms = None;
            current.next_ack_poll_at_unix_ms = None;
        })
        .await;
    }

    let failures = record.delivery_attempts.max(1).saturating_sub(1);
    let multiplier = 1u64.checked_shl(failures.min(16)).unwrap_or(u64::MAX);
    let delay_seconds = link
        .initial_retry_delay_seconds
        .saturating_mul(multiplier)
        .min(link.max_retry_delay_seconds);
    let due = now.saturating_add(delay_seconds.saturating_mul(1_000));
    let next_delivery_at = next_delivery_after(link, due)?;
    let next_state = if next_delivery_at == due {
        RelayQueueState::Retrying
    } else {
        RelayQueueState::BlockedByBlackout
    };

    update_record(app, record, move |current| {
        current.state = next_state;
        current.last_delivery_error = Some(error);
        current.next_delivery_at_unix_ms = Some(next_delivery_at);
        current.next_ack_poll_at_unix_ms = None;
    })
    .await
}

async fn update_record<F>(
    app: &Arc<AppState>,
    record: &RelayPackageRecord,
    mutator: F,
) -> anyhow::Result<()>
where
    F: FnOnce(&mut RelayPackageRecord),
{
    let mut index = app.index.lock().await;
    let Some(current) = index
        .record(
            record.source_domain,
            record.target_domain,
            record.epoch_id,
            decode_hex_hash(&record.package_hash)?,
        )
        .cloned()
    else {
        return Ok(());
    };
    let mut updated = current;
    mutator(&mut updated);
    app.store.persist_record(&mut index, updated)?;
    app.store.save_index(&index)?;
    Ok(())
}

fn decode_and_validate_envelope(
    bytes: &[u8],
) -> anyhow::Result<(RelayPackageEnvelopeV1, CertifiedSummaryPackage)> {
    let envelope = RelayPackageEnvelopeV1::decode(&mut &bytes[..])
        .map_err(|error| anyhow!("failed to decode relay package envelope: {error}"))?;
    let package = CertifiedSummaryPackage::decode(&mut &envelope.package_bytes[..])
        .map_err(|error| anyhow!("failed to decode certified summary package: {error}"))?;

    if package.compute_package_hash() != package.package_hash {
        bail!("package_hash does not match certified summary package contents");
    }
    if package.package_hash != envelope.package_hash {
        bail!("envelope package_hash does not match package bytes");
    }
    if package.header.domain_id != envelope.source_domain {
        bail!("envelope source_domain does not match package source domain");
    }
    if package.header.epoch_id != envelope.epoch_id {
        bail!("envelope epoch_id does not match package header epoch");
    }
    if package.header.summary_hash != envelope.summary_hash {
        bail!("envelope summary_hash does not match package header summary_hash");
    }

    let (target_domain, export_count) = derive_target_domain(&package)?;
    if target_domain != envelope.target_domain {
        bail!("envelope target_domain does not match package export proofs");
    }
    if export_count != envelope.export_count {
        bail!("envelope export_count does not match package export proof count");
    }

    Ok((envelope, package))
}

fn derive_target_domain(
    package: &CertifiedSummaryPackage,
) -> anyhow::Result<(ialp_common_types::DomainId, u32)> {
    if package.inclusion_proofs.len() < 2 {
        bail!("transport packages must carry at least one export proof");
    }
    let mut target_domain = None;
    let mut export_count = 0u32;
    for (index, encoded) in package.inclusion_proofs.iter().enumerate() {
        let proof = InclusionProof::decode(&mut &encoded[..])
            .map_err(|error| anyhow!("failed to decode package inclusion proof: {error}"))?;
        match (index, proof) {
            (0, InclusionProof::SummaryHeaderStorageV1(_)) => {}
            (0, InclusionProof::ExportV1(_)) => {
                bail!("summary-header storage proof must remain at inclusion_proofs[0]")
            }
            (_, InclusionProof::SummaryHeaderStorageV1(_)) => {
                bail!("summary-header storage proof may appear only at inclusion_proofs[0]")
            }
            (_, InclusionProof::ExportV1(proof)) => {
                export_count = export_count.saturating_add(1);
                match target_domain {
                    Some(existing) if existing != proof.leaf.target_domain => {
                        bail!("package export proofs contain mixed target domains")
                    }
                    None => target_domain = Some(proof.leaf.target_domain),
                    _ => {}
                }
                if proof.leaf.source_domain != package.header.domain_id
                    || proof.leaf.source_epoch_id != package.header.epoch_id
                {
                    bail!("package export proofs do not match package source domain/epoch");
                }
            }
        }
    }
    match target_domain {
        Some(target_domain) => Ok((target_domain, export_count)),
        None => bail!("package does not contain any export proofs"),
    }
}

fn normalize_restart_state(
    index: &mut RelayIndex,
    ack_poll_millis: u64,
    now: u64,
) -> anyhow::Result<()> {
    for record in &mut index.packages {
        match record.state {
            RelayQueueState::InDelivery => {
                record.state = RelayQueueState::Retrying;
                record.next_delivery_at_unix_ms = Some(now);
            }
            RelayQueueState::Delivered if record.next_ack_poll_at_unix_ms.is_none() => {
                record.next_ack_poll_at_unix_ms = Some(now.saturating_add(ack_poll_millis));
            }
            _ => {}
        }
    }
    Ok(())
}

fn next_delivery_after(link: &LinkProfileConfig, unix_ms: u64) -> anyhow::Result<u64> {
    let mut candidate = unix_ms_to_utc(unix_ms)?;
    loop {
        let mut advanced = false;
        for window in &link.blackout_windows {
            if window.start <= candidate && candidate < window.end {
                candidate = window.end;
                advanced = true;
            }
        }
        if !advanced {
            return utc_to_unix_millis(candidate);
        }
    }
}

fn importer_base_url(
    transport: &TransportConfig,
    target_domain: ialp_common_types::DomainId,
) -> anyhow::Result<String> {
    let importer = transport
        .importers
        .get(&target_domain)
        .ok_or_else(|| anyhow!("missing importer endpoint for {}", target_domain))?;
    Ok(format!("http://{}", importer.listen_addr))
}

fn due_now(next_due_at_unix_ms: Option<u64>, now: u64) -> bool {
    next_due_at_unix_ms.map(|due| now >= due).unwrap_or(false)
}

fn relay_state_name(state: &RelayQueueState) -> &'static str {
    match state {
        RelayQueueState::Queued => "queued",
        RelayQueueState::Scheduled => "scheduled",
        RelayQueueState::BlockedByBlackout => "blocked_by_blackout",
        RelayQueueState::InDelivery => "in_delivery",
        RelayQueueState::Delivered => "delivered",
        RelayQueueState::ImporterAcked => "importer_acked",
        RelayQueueState::Retrying => "retrying",
        RelayQueueState::Failed => "failed",
    }
}

fn unix_now_millis() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is set before unix epoch")?
        .as_millis()
        .try_into()
        .context("unix timestamp does not fit into u64")
}

fn unix_ms_to_utc(unix_ms: u64) -> anyhow::Result<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp_millis(i64::try_from(unix_ms)?)
        .ok_or_else(|| anyhow!("timestamp {} is out of range", unix_ms))
}

fn utc_to_unix_millis(timestamp: DateTime<Utc>) -> anyhow::Result<u64> {
    timestamp
        .timestamp_millis()
        .try_into()
        .map_err(|_| anyhow!("timestamp {} does not fit into u64", timestamp))
}

fn render_json(value: serde_json::Value, _as_json: bool) {
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("json output should serialize")
    );
}

impl RelaySettings {
    fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let loaded = load_transport_config(path.as_deref())
            .context("failed to load relay transport config")?;
        Ok(Self {
            listen_addr: loaded.config.relay.listen_addr.clone(),
            store_dir: loaded.config.relay.store_dir.clone(),
            scheduler_tick_millis: loaded.config.relay.scheduler_tick_millis,
            ack_poll_millis: loaded.config.relay.ack_poll_millis,
            transport: loaded.config,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::Encode;
    use ialp_common_types::{
        DomainId, EpochSummaryHeader, ExportInclusionProof, ExportLeaf, ExportLeafHashInput,
        GrandpaFinalityCertificate, SummaryCertificate, SummaryCertificationBundle,
        SummaryHeaderStorageProof, SUMMARY_HEADER_STORAGE_PROOF_VERSION,
    };

    fn sample_package(target_domain: DomainId) -> CertifiedSummaryPackage {
        let leaf = ExportLeaf::from_hash_input(ExportLeafHashInput {
            version: 1,
            export_id: [7u8; 32],
            source_domain: DomainId::Earth,
            target_domain,
            sender: [1u8; 32],
            recipient: [2u8; 32],
            amount: 5,
            source_epoch_id: 4,
            source_block_height: 10,
            extrinsic_index: 0,
        });
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
            export_root: ialp_common_types::export_merkle_root(
                DomainId::Earth,
                4,
                9,
                12,
                core::slice::from_ref(&leaf),
            ),
            import_root: [5u8; 32],
            governance_root: [6u8; 32],
            validator_set_hash: [8u8; 32],
            summary_hash: [9u8; 32],
        };
        let bundle = SummaryCertificationBundle {
            certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                version: 1,
                grandpa_set_id: 0,
                target_block_number: 13,
                target_block_hash: [10u8; 32],
                proof_block_number: 14,
                proof_block_hash: [11u8; 32],
                justification: Vec::new(),
                ancestry_headers: Vec::new(),
            }),
            summary_header_storage_proof: SummaryHeaderStorageProof {
                version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                proof_block_number: 14,
                proof_block_hash: [11u8; 32],
                proof_block_header: Vec::new(),
                storage_key: vec![1],
                trie_nodes: Vec::new(),
            },
        };
        CertifiedSummaryPackage::from_bundle_with_export_proofs(
            header,
            bundle,
            vec![ExportInclusionProof {
                version: ialp_common_types::EXPORT_INCLUSION_PROOF_VERSION,
                leaf,
                leaf_index: 0,
                leaf_count: 1,
                siblings: Vec::new(),
            }],
        )
    }

    #[test]
    fn envelope_validation_rejects_target_domain_mismatch() {
        let package = sample_package(DomainId::Moon);
        let envelope = RelayPackageEnvelopeV1::new(
            DomainId::Earth,
            DomainId::Mars,
            4,
            package.header.summary_hash,
            package.package_hash,
            package.encode(),
            1,
            100,
        );

        let error = decode_and_validate_envelope(&envelope.encode()).expect_err("should fail");
        assert!(error.to_string().contains("target_domain"));
    }

    #[test]
    fn blackout_windows_use_half_open_intervals() {
        let link = LinkProfileConfig {
            source_domain: DomainId::Earth,
            target_domain: DomainId::Moon,
            base_one_way_delay_seconds: 1,
            initial_retry_delay_seconds: 1,
            max_retry_delay_seconds: 4,
            max_attempts: 0,
            blackout_windows: vec![ialp_common_config::BlackoutWindowConfig {
                start: DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                    .expect("start")
                    .with_timezone(&Utc),
                end: DateTime::parse_from_rfc3339("2026-01-01T00:10:00Z")
                    .expect("end")
                    .with_timezone(&Utc),
            }],
        };

        let blocked = next_delivery_after(
            &link,
            utc_to_unix_millis(
                DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                    .expect("blocked")
                    .with_timezone(&Utc),
            )
            .expect("unix"),
        )
        .expect("deferred");
        let allowed = next_delivery_after(
            &link,
            utc_to_unix_millis(
                DateTime::parse_from_rfc3339("2026-01-01T00:10:00Z")
                    .expect("allowed")
                    .with_timezone(&Utc),
            )
            .expect("unix"),
        )
        .expect("allowed");

        assert!(blocked > allowed.saturating_sub(1));
        assert_eq!(blocked, allowed);
    }
}
