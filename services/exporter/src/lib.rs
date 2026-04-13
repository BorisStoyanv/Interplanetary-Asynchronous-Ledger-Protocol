mod cli;
mod rpc_client;
mod store;

use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{bail, Context};
use clap::Parser;
use ialp_common_config::load_domain_config;
use ialp_common_types::{
    build_export_inclusion_proof, export_merkle_root, sort_export_leaves, CertifiedSummaryPackage,
    DomainId, ExportLeaf, SummaryCertificate, SummaryCertificationReadiness,
    SummaryCertificationState,
};

use crate::{
    cli::{Cli, Commands},
    rpc_client::{NodeRpcClient, StagedSummaryView},
    store::{decode_export_proofs, decode_summary_header_storage_proof, PackageRecord, Store},
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
        let records = if let Some(target_domain) = args.target_domain {
            index
                .record(epoch_id, target_domain)
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            index.records_for_epoch(epoch_id)
        };
        let status = serde_json::json!({
            "domain": settings.domain,
            "node_url": settings.node_url,
            "staged": staged.as_ref().map(StagedSummaryView::json_summary),
            "readiness": readiness.as_ref().map(readiness_json),
            "packages": records.into_iter().map(PackageRecord::json_summary).collect::<Vec<_>>(),
        });
        render_json(status, args.json);
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
    render_json(status, args.json);
    Ok(())
}

fn show_package(args: cli::ShowArgs) -> anyhow::Result<()> {
    let store = Store::new(args.store_dir, args.domain)?;
    let (manifest, package) = store.load_package(args.epoch, args.target_domain)?;
    let summary_proof = decode_summary_header_storage_proof(
        package
            .inclusion_proofs
            .first()
            .context("package is missing inclusion_proofs[0]")?,
    )?;
    let export_proofs = decode_export_proofs(&package.inclusion_proofs[1..])?;

    let output = serde_json::json!({
        "manifest": manifest,
        "package": {
            "version": package.version,
            "package_hash": hex_hash(package.package_hash),
            "source_domain": package.header.domain_id,
            "epoch_id": package.header.epoch_id,
            "summary_hash": hex_hash(package.header.summary_hash),
            "export_root": hex_hash(package.header.export_root),
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
            "inclusion_proofs": {
                "summary_header_storage_v1": {
                    "proof_block_number": summary_proof.proof_block_number,
                    "proof_block_hash": hex_hash(summary_proof.proof_block_hash),
                    "storage_key": hex_bytes(&summary_proof.storage_key),
                    "proof_node_count": summary_proof.node_count(),
                    "proof_total_bytes": summary_proof.total_proof_bytes(),
                    "proof_block_header_len": summary_proof.proof_block_header.len(),
                },
                "export_v1": export_proofs.iter().map(|proof| serde_json::json!({
                    "export_id": hex_hash(proof.leaf.export_id),
                    "source_domain": proof.leaf.source_domain,
                    "target_domain": proof.leaf.target_domain,
                    "amount": proof.leaf.amount,
                    "leaf_index": proof.leaf_index,
                    "leaf_count": proof.leaf_count,
                    "sibling_count": proof.siblings.len(),
                })).collect::<Vec<_>>(),
            },
            "artifacts": {
                "count": package.artifacts.len(),
            },
        },
    });

    render_json(output, args.json);
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
        let mut leaves = rpc
            .epoch_exports(epoch_id)
            .await?
            .into_iter()
            .map(|record| record.leaf)
            .collect::<Vec<_>>();
        let expected_root = export_merkle_root(
            staged.header.domain_id,
            staged.header.epoch_id,
            staged.header.start_block_height,
            staged.header.end_block_height,
            &leaves,
        );
        if expected_root != staged.header.export_root {
            bail!(
                "epoch {} export_root mismatch: header={} exporter={}",
                epoch_id,
                hex_hash(staged.header.export_root),
                hex_hash(expected_root),
            );
        }

        sort_export_leaves(&mut leaves);
        let grouped = group_exports_by_target(&leaves);
        for (target_domain, target_leaves) in &grouped {
            let export_ids = target_leaves
                .iter()
                .map(|leaf| leaf.export_id)
                .collect::<Vec<_>>();
            let pending_reason = match &readiness.state {
                SummaryCertificationState::Pending(reason) => Some(format!("{reason:?}")),
                SummaryCertificationState::Ready(_) => None,
            };
            index.upsert_pending(
                epoch_id,
                *target_domain,
                staged.header.summary_hash,
                staged.staged_at_block_number,
                readiness.staged_at_block_hash,
                &export_ids,
                pending_reason.clone(),
            );
        }

        if let SummaryCertificationState::Ready(bundle) = readiness.state {
            for (target_domain, target_leaves) in grouped {
                let export_ids = target_leaves
                    .iter()
                    .map(|leaf| leaf.export_id)
                    .collect::<Vec<_>>();
                let export_proofs = target_leaves
                    .iter()
                    .map(|leaf| {
                        build_export_inclusion_proof(&leaves, leaf.export_id).ok_or_else(|| {
                            anyhow::anyhow!(
                                "missing export proof for {} in epoch {}",
                                hex::encode(leaf.export_id),
                                epoch_id
                            )
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let package = CertifiedSummaryPackage::from_bundle_with_export_proofs(
                    staged.header.clone(),
                    bundle.clone(),
                    export_proofs,
                );
                if package.inclusion_proofs.is_empty() || !package.artifacts.is_empty() {
                    bail!("Phase 2B package builder emitted invalid proof/artifact layout");
                }
                store.persist_package(&mut index, target_domain, &export_ids, &package)?;
            }
        }
    }

    store.save_index(&index)?;
    Ok(())
}

fn group_exports_by_target(leaves: &[ExportLeaf]) -> BTreeMap<DomainId, Vec<ExportLeaf>> {
    let mut grouped = BTreeMap::new();
    for leaf in leaves {
        grouped
            .entry(leaf.target_domain)
            .or_insert_with(Vec::new)
            .push(leaf.clone());
    }
    grouped
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
                    "storage_key": hex_bytes(&bundle.summary_header_storage_proof.storage_key),
                    "proof_node_count": bundle.summary_header_storage_proof.node_count(),
                    "proof_total_bytes": bundle.summary_header_storage_proof.total_proof_bytes(),
                    "proof_block_header_len": bundle.summary_header_storage_proof.proof_block_header.len(),
                },
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

fn render_json(value: serde_json::Value, _as_json: bool) {
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("json output should serialize")
    );
}

fn hex_hash(hash: [u8; 32]) -> String {
    format!("0x{}", hex::encode(hash))
}

fn hex_bytes(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
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
    use codec::Decode;
    use ialp_common_types::{
        DomainId, EpochSummaryHeader, GrandpaFinalityCertificate, InclusionProof,
        SummaryCertificate, SummaryCertificationBundle, SummaryCertificationState,
        SummaryHeaderStorageProof, SUMMARY_HEADER_STORAGE_PROOF_VERSION,
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
            export_root: ialp_common_types::export_merkle_root(
                DomainId::Earth,
                3,
                10,
                12,
                &[
                    ExportLeaf::from_hash_input(ialp_common_types::ExportLeafHashInput {
                        version: 1,
                        export_id: [21u8; 32],
                        source_domain: DomainId::Earth,
                        target_domain: DomainId::Moon,
                        sender: [22u8; 32],
                        recipient: [23u8; 32],
                        amount: 100,
                        source_epoch_id: 3,
                        source_block_height: 11,
                        extrinsic_index: 0,
                    }),
                    ExportLeaf::from_hash_input(ialp_common_types::ExportLeafHashInput {
                        version: 1,
                        export_id: [24u8; 32],
                        source_domain: DomainId::Earth,
                        target_domain: DomainId::Mars,
                        sender: [25u8; 32],
                        recipient: [26u8; 32],
                        amount: 200,
                        source_epoch_id: 3,
                        source_block_height: 11,
                        extrinsic_index: 1,
                    }),
                ],
            ),
            import_root: [7u8; 32],
            governance_root: [8u8; 32],
            validator_set_hash: [9u8; 32],
            summary_hash: [10u8; 32],
        }
    }

    fn sample_bundle() -> SummaryCertificationBundle {
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
        }
    }

    #[test]
    fn groups_exports_by_target_domain_without_losing_canonical_order() {
        let mut leaves = vec![
            ExportLeaf::from_hash_input(ialp_common_types::ExportLeafHashInput {
                version: 1,
                export_id: [2u8; 32],
                source_domain: DomainId::Earth,
                target_domain: DomainId::Moon,
                sender: [0u8; 32],
                recipient: [0u8; 32],
                amount: 1,
                source_epoch_id: 0,
                source_block_height: 2,
                extrinsic_index: 0,
            }),
            ExportLeaf::from_hash_input(ialp_common_types::ExportLeafHashInput {
                version: 1,
                export_id: [1u8; 32],
                source_domain: DomainId::Earth,
                target_domain: DomainId::Moon,
                sender: [0u8; 32],
                recipient: [0u8; 32],
                amount: 1,
                source_epoch_id: 0,
                source_block_height: 1,
                extrinsic_index: 1,
            }),
        ];
        sort_export_leaves(&mut leaves);
        let grouped = group_exports_by_target(&leaves);
        let moon = grouped.get(&DomainId::Moon).expect("moon exports");
        assert_eq!(moon[0].source_block_height, 1);
        assert_eq!(moon[1].source_block_height, 2);
    }

    #[test]
    fn package_builder_keeps_summary_proof_first_and_export_proofs_after() {
        let leaves = vec![
            ExportLeaf::from_hash_input(ialp_common_types::ExportLeafHashInput {
                version: 1,
                export_id: [31u8; 32],
                source_domain: DomainId::Earth,
                target_domain: DomainId::Moon,
                sender: [0u8; 32],
                recipient: [1u8; 32],
                amount: 10,
                source_epoch_id: 3,
                source_block_height: 11,
                extrinsic_index: 0,
            }),
            ExportLeaf::from_hash_input(ialp_common_types::ExportLeafHashInput {
                version: 1,
                export_id: [32u8; 32],
                source_domain: DomainId::Earth,
                target_domain: DomainId::Moon,
                sender: [0u8; 32],
                recipient: [1u8; 32],
                amount: 20,
                source_epoch_id: 3,
                source_block_height: 11,
                extrinsic_index: 1,
            }),
        ];
        let package = CertifiedSummaryPackage::from_bundle_with_export_proofs(
            sample_header(),
            sample_bundle(),
            vec![
                build_export_inclusion_proof(&leaves, [31u8; 32]).expect("proof"),
                build_export_inclusion_proof(&leaves, [32u8; 32]).expect("proof"),
            ],
        );

        assert!(matches!(
            SummaryCertificationState::Ready(sample_bundle()).clone(),
            SummaryCertificationState::Ready(_)
        ));
        assert_eq!(package.inclusion_proofs.len(), 3);
        assert!(matches!(
            InclusionProof::decode(&mut &package.inclusion_proofs[0][..]).expect("proof"),
            InclusionProof::SummaryHeaderStorageV1(_)
        ));
        assert!(matches!(
            InclusionProof::decode(&mut &package.inclusion_proofs[1][..]).expect("proof"),
            InclusionProof::ExportV1(_)
        ));
    }
}
