mod cli;
mod rpc_client;
mod store;

use std::path::PathBuf;

use anyhow::{bail, Context};
use clap::Parser;
use codec::Decode;
use ialp_common_config::load_domain_config;
use ialp_common_types::{
    CertifiedSummaryPackage, DomainId, InclusionProof, SummaryCertificate,
    SummaryCertificationReadiness, SummaryCertificationState,
};

use crate::{
    cli::{Cli, Commands},
    rpc_client::{NodeRpcClient, StagedSummaryView},
    store::{PackageRecord, Store},
};

pub async fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => run_exporter(args).await,
        Commands::Status(args) => show_status(args).await,
        Commands::Show(args) => show_package(args),
    }
}

async fn run_exporter(args: cli::RunArgs) -> anyhow::Result<()> {
    let settings = ExporterSettings::load(args.domain, args.config, args.node_url, args.store_dir)?;
    let rpc = NodeRpcClient::connect(&settings.node_url).await?;
    let store = Store::new(settings.store_dir.clone(), settings.domain)?;

    sync_once(&rpc, &store).await?;

    let mut finalized_heads = rpc.subscribe_finalized_heads().await?;
    while finalized_heads.next().await.transpose()?.is_some() {
        sync_once(&rpc, &store).await?;
    }

    Ok(())
}

async fn show_status(args: cli::StatusArgs) -> anyhow::Result<()> {
    let settings = ExporterSettings::load(args.domain, args.config, args.node_url, args.store_dir)?;
    let rpc = NodeRpcClient::connect(&settings.node_url).await?;
    let store = Store::new(settings.store_dir.clone(), settings.domain)?;
    let index = store.load_index()?;

    let latest_staged = rpc.latest_staged_summary().await?;
    let latest_certified = index.latest_certified_record();

    if let Some(epoch_id) = args.epoch {
        let staged = rpc.summary_by_epoch(epoch_id).await?;
        let readiness = if staged.is_some() {
            Some(rpc.certification_readiness(epoch_id).await?)
        } else {
            None
        };
        let certified = index.record(epoch_id).cloned();
        let status = serde_json::json!({
            "domain": settings.domain,
            "node_url": settings.node_url,
            "staged": staged.as_ref().map(StagedSummaryView::json_summary),
            "readiness": readiness.as_ref().map(readiness_json),
            "certified": certified.as_ref().map(PackageRecord::json_summary),
        });
        render_status(status, args.json);
        return Ok(());
    }

    let latest_readiness = match latest_staged.as_ref() {
        Some(staged) => Some(rpc.certification_readiness(staged.header.epoch_id).await?),
        None => None,
    };

    let status = serde_json::json!({
        "domain": settings.domain,
        "node_url": settings.node_url,
        "latest_staged": latest_staged.as_ref().map(StagedSummaryView::json_summary),
        "latest_staged_readiness": latest_readiness.as_ref().map(readiness_json),
        "latest_certified": latest_certified.map(PackageRecord::json_summary),
        "store_dir": settings.store_dir,
        "index": index.json_summary(),
    });
    render_status(status, args.json);
    Ok(())
}

fn show_package(args: cli::ShowArgs) -> anyhow::Result<()> {
    let store = Store::new(args.store_dir, args.domain)?;
    let (manifest, package) = store.load_package(args.epoch)?;
    let summary_header_storage_proof = decode_summary_header_storage_proof(
        package
            .inclusion_proofs
            .first()
            .context("Phase 2A package is missing inclusion_proofs[0]")?,
    )?;

    let output = serde_json::json!({
        "manifest": manifest,
        "package": {
            "version": package.version,
            "package_hash": hex_hash(package.package_hash),
            "domain_id": package.header.domain_id,
            "epoch_id": package.header.epoch_id,
            "summary_hash": hex_hash(package.header.summary_hash),
            "certificate": match package.certificate {
                SummaryCertificate::GrandpaV1(ref certificate) => serde_json::json!({
                    "version": certificate.version,
                    "grandpa_set_id": certificate.grandpa_set_id,
                    "target_block_number": certificate.target_block_number,
                    "target_block_hash": hex_hash(certificate.target_block_hash),
                    "proof_block_number": certificate.proof_block_number,
                    "proof_block_hash": hex_hash(certificate.proof_block_hash),
                    "justification_len": certificate.justification.len(),
                    "ancestry_header_count": certificate.ancestry_headers.len(),
                }),
            },
            "inclusion_proofs": [{
                "kind": "summary_header_storage_v1",
                "proof_block_number": summary_header_storage_proof.proof_block_number,
                "proof_block_hash": hex_hash(summary_header_storage_proof.proof_block_hash),
                "storage_key": hex_bytes(&summary_header_storage_proof.storage_key),
                "proof_node_count": summary_header_storage_proof.node_count(),
                "proof_total_bytes": summary_header_storage_proof.total_proof_bytes(),
                "proof_block_header_len": summary_header_storage_proof.proof_block_header.len(),
            }],
            "artifacts": {
                "count": package.artifacts.len(),
            },
        },
    });

    render_status(output, args.json);
    Ok(())
}

async fn sync_once(rpc: &NodeRpcClient, store: &Store) -> anyhow::Result<()> {
    let latest_staged = rpc.latest_staged_summary().await?;
    let mut index = store.load_index()?;

    index.latest_staged_epoch = latest_staged
        .as_ref()
        .map(|summary| summary.header.epoch_id);

    let Some(latest_staged) = latest_staged else {
        store.save_index(&index)?;
        return Ok(());
    };

    for epoch_id in 0..=latest_staged.header.epoch_id {
        let Some(staged) = rpc.summary_by_epoch(epoch_id).await? else {
            continue;
        };

        let readiness = rpc.certification_readiness(epoch_id).await?;
        let pending = match readiness.state {
            SummaryCertificationState::Pending(ref reason) => Some(*reason),
            SummaryCertificationState::Ready(_) => None,
        };

        index.upsert_pending(
            staged.header.epoch_id,
            staged.header.summary_hash,
            staged.staged_at_block_number,
            readiness.staged_at_block_hash,
            pending,
        );

        if let SummaryCertificationState::Ready(bundle) = readiness.state {
            let package = CertifiedSummaryPackage::from_bundle(staged.header.clone(), bundle);
            if package.inclusion_proofs.len() != 1 || !package.artifacts.is_empty() {
                bail!(
                    "Phase 2A package builder must emit exactly one inclusion proof and zero artifacts"
                );
            }
            store.persist_package(&mut index, &package)?;
        }
    }

    store.save_index(&index)?;
    Ok(())
}

fn readiness_json(readiness: &SummaryCertificationReadiness) -> serde_json::Value {
    let state = match &readiness.state {
        SummaryCertificationState::Pending(reason) => serde_json::json!({
            "status": "pending",
            "reason": format!("{reason:?}"),
        }),
        SummaryCertificationState::Ready(bundle) => {
            let SummaryCertificate::GrandpaV1(certificate) = &bundle.certificate;
            serde_json::json!({
                "status": "ready",
                "grandpa_set_id": certificate.grandpa_set_id,
                "target_block_number": certificate.target_block_number,
                "target_block_hash": hex_hash(certificate.target_block_hash),
                "proof_block_number": certificate.proof_block_number,
                "proof_block_hash": hex_hash(certificate.proof_block_hash),
                "justification_len": certificate.justification.len(),
                "ancestry_header_count": certificate.ancestry_headers.len(),
                "summary_header_storage_proof": {
                    "kind": "summary_header_storage_v1",
                    "storage_key": hex_bytes(&bundle.summary_header_storage_proof.storage_key),
                    "proof_node_count": bundle.summary_header_storage_proof.node_count(),
                    "proof_total_bytes": bundle.summary_header_storage_proof.total_proof_bytes(),
                    "proof_block_header_len": bundle.summary_header_storage_proof.proof_block_header.len(),
                }
            })
        }
    };

    serde_json::json!({
        "epoch_id": readiness.epoch_id,
        "staged_at_block_number": readiness.staged_at_block_number,
        "staged_at_block_hash": hex_hash(readiness.staged_at_block_hash),
        "latest_finalized_block_number": readiness.latest_finalized_block_number,
        "latest_finalized_block_hash": hex_hash(readiness.latest_finalized_block_hash),
        "state": state,
    })
}

fn render_status(value: serde_json::Value, as_json: bool) {
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("json output should serialize")
        );
        return;
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("status output should serialize")
    );
}

fn hex_hash(hash: [u8; 32]) -> String {
    format!("0x{}", hex::encode(hash))
}

fn hex_bytes(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn decode_summary_header_storage_proof(
    bytes: &[u8],
) -> anyhow::Result<ialp_common_types::SummaryHeaderStorageProof> {
    let proof = InclusionProof::decode(&mut &bytes[..])
        .map_err(|error| anyhow::anyhow!("failed to decode inclusion proof: {error}"))?;
    match proof {
        InclusionProof::SummaryHeaderStorageV1(proof) => Ok(proof),
    }
}

struct ExporterSettings {
    domain: DomainId,
    node_url: String,
    store_dir: PathBuf,
}

impl ExporterSettings {
    fn load(
        domain: DomainId,
        config: Option<PathBuf>,
        node_url: Option<String>,
        store_dir: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let loaded = load_domain_config(domain, config.as_deref())
            .with_context(|| format!("failed to load exporter config for domain {domain}"))?;
        let node_url = node_url
            .unwrap_or_else(|| format!("ws://127.0.0.1:{}", loaded.config.network.rpc_port));
        let store_dir =
            store_dir.unwrap_or_else(|| PathBuf::from("var/exporter").join(domain.as_str()));

        Ok(Self {
            domain,
            node_url,
            store_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ialp_common_types::{
        CertificationPendingReason, DomainId, EpochSummaryHeader, GrandpaFinalityCertificate,
        SummaryCertificationBundle, SummaryCertificationReadiness, SummaryCertificationState,
        SummaryHeaderStorageProof, EMPTY_HASH, SUMMARY_HEADER_STORAGE_PROOF_VERSION,
    };

    fn sample_header() -> EpochSummaryHeader {
        EpochSummaryHeader {
            version: 1,
            domain_id: DomainId::Earth,
            epoch_id: 3,
            prev_summary_hash: [1u8; 32],
            start_block_height: 10,
            end_block_height: 12,
            state_root: [2u8; 32],
            block_root: [3u8; 32],
            tx_root: [4u8; 32],
            event_root: [5u8; 32],
            export_root: [6u8; 32],
            import_root: [7u8; 32],
            governance_root: [8u8; 32],
            validator_set_hash: [9u8; 32],
            summary_hash: [10u8; 32],
        }
    }

    #[test]
    fn package_builder_emits_single_phase_2a_inclusion_proof() {
        let package = CertifiedSummaryPackage::from_bundle(
            sample_header(),
            SummaryCertificationBundle {
                certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                    version: 1,
                    grandpa_set_id: 0,
                    target_block_number: 13,
                    target_block_hash: [11u8; 32],
                    proof_block_number: 13,
                    proof_block_hash: [11u8; 32],
                    justification: vec![1, 2, 3],
                    ancestry_headers: Vec::new(),
                }),
                summary_header_storage_proof: SummaryHeaderStorageProof {
                    version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                    proof_block_number: 13,
                    proof_block_hash: [11u8; 32],
                    proof_block_header: vec![9, 9],
                    storage_key: ialp_common_types::summary_header_storage_key(3),
                    trie_nodes: vec![vec![1, 2, 3]],
                },
            },
        );

        assert_eq!(package.inclusion_proofs.len(), 1);
        assert!(package.artifacts.is_empty());
    }

    #[test]
    fn readiness_json_preserves_pending_reason() {
        let readiness = SummaryCertificationReadiness {
            epoch_id: 9,
            staged_at_block_number: 22,
            staged_at_block_hash: EMPTY_HASH,
            latest_finalized_block_number: 21,
            latest_finalized_block_hash: EMPTY_HASH,
            state: SummaryCertificationState::Pending(
                CertificationPendingReason::TargetBlockNotFinalized,
            ),
        };

        let json = readiness_json(&readiness);
        assert_eq!(json["state"]["reason"], "TargetBlockNotFinalized");
    }

    #[test]
    fn readiness_json_surfaces_storage_proof_metadata() {
        let readiness = SummaryCertificationReadiness {
            epoch_id: 9,
            staged_at_block_number: 22,
            staged_at_block_hash: [1u8; 32],
            latest_finalized_block_number: 24,
            latest_finalized_block_hash: [2u8; 32],
            state: SummaryCertificationState::Ready(SummaryCertificationBundle {
                certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                    version: 1,
                    grandpa_set_id: 0,
                    target_block_number: 22,
                    target_block_hash: [1u8; 32],
                    proof_block_number: 24,
                    proof_block_hash: [3u8; 32],
                    justification: vec![1, 2],
                    ancestry_headers: vec![vec![4, 5]],
                }),
                summary_header_storage_proof: SummaryHeaderStorageProof {
                    version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                    proof_block_number: 24,
                    proof_block_hash: [3u8; 32],
                    proof_block_header: vec![7, 8, 9],
                    storage_key: ialp_common_types::summary_header_storage_key(9),
                    trie_nodes: vec![vec![1], vec![2, 3]],
                },
            }),
        };

        let json = readiness_json(&readiness);
        assert_eq!(
            json["state"]["summary_header_storage_proof"]["proof_node_count"],
            2
        );
        assert_eq!(
            json["state"]["summary_header_storage_proof"]["storage_key"],
            hex_bytes(&ialp_common_types::summary_header_storage_key(9))
        );
    }
}
