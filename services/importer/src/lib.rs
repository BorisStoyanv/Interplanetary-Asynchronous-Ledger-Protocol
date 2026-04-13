mod cli;
mod rpc_client;
mod store;

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context};
use clap::Parser;
use codec::{Decode, Encode};
use ialp_common_config::load_domain_config;
use ialp_common_types::{
    summary_header_storage_key, CertifiedSummaryPackage, DomainId, InclusionProof,
    ObservedImportClaim, SummaryCertificate,
};
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

use crate::{
    cli::{parse_export_id_hex, Cli, Commands},
    rpc_client::{load_submitter_pair, NodeRpcClient},
    store::{DuplicateStatus, ImportRecord, Store, SubmissionStatus, VerificationStatus},
};

pub async fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Verify(args) => verify_package_command(args).await,
        Commands::Status(args) => show_status(args),
        Commands::Show(args) => show_record(args),
    }
}

async fn verify_package_command(args: cli::VerifyArgs) -> anyhow::Result<()> {
    let settings = ImporterSettings::load(args.domain, args.node_url, args.store_dir.clone())?;
    let rpc = NodeRpcClient::connect(&settings.node_url).await?;
    let mut index = settings.store.load_index()?;
    let package_bytes = std::fs::read(&args.package)
        .with_context(|| format!("failed to read package {}", args.package.display()))?;
    let package = CertifiedSummaryPackage::decode(&mut &package_bytes[..])
        .map_err(|error| anyhow!("failed to decode certified summary package: {error}"))?;

    verify_package_hash(&package)?;
    let certificate = verify_summary_chain_context(&package)?;
    verify_grandpa_certificate(&certificate, package.header.domain_id)?;
    let export_proofs = verify_export_proofs(&package, args.domain)?;

    let submitter_pair = load_submitter_pair(&args.submitter_suri)?;
    let submitter_account = submitter_account_id(&submitter_pair);
    let configured_importer = rpc
        .importer_account()
        .await?
        .ok_or_else(|| anyhow!("destination chain is missing configured importer account"))?;
    if submitter_account != configured_importer {
        bail!(
            "submitter account {} does not match allowlisted importer account {}",
            submitter_account.to_ss58check(),
            configured_importer.to_ss58check()
        );
    }

    let runtime_version = rpc.runtime_version().await?;
    ensure_runtime_version_matches(&runtime_version)?;
    let genesis_hash = rpc.genesis_hash().await?;

    let mut results = Vec::new();
    for proof in export_proofs {
        let export_id = proof.leaf.export_id;
        if let Some(existing) = index.record(export_id).cloned() {
            results.push(existing);
            continue;
        }
        if rpc.observed_import(export_id).await?.is_some() {
            let record = build_duplicate_record(
                &package,
                &proof,
                DuplicateStatus::DuplicateRemote,
                SubmissionStatus::SkippedDuplicate,
                Some("export_id already observed on destination chain".into()),
            );
            settings.store.persist_record(&mut index, record.clone())?;
            results.push(record);
            continue;
        }

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
            &submitter_pair,
            submitter_account.clone(),
            runtime_version.spec_version,
            runtime_version.transaction_version,
            genesis_hash,
            rpc.account_next_index(&submitter_account).await?,
            claim.clone(),
        )?;
        let tx_hash = rpc.submit_extrinsic(extrinsic).await?;

        let record = ImportRecord {
            export_id: hex_hash(export_id),
            source_domain: proof.leaf.source_domain,
            target_domain: proof.leaf.target_domain,
            summary_hash: hex_hash(package.header.summary_hash),
            package_hash: hex_hash(package.package_hash),
            verification_status: VerificationStatus::Verified,
            duplicate_status: DuplicateStatus::NotDuplicate,
            submission_status: SubmissionStatus::RemoteObserved,
            tx_hash: Some(tx_hash),
            reason: Some("remote_observed recorded".into()),
        };
        settings.store.persist_record(&mut index, record.clone())?;
        results.push(record);
    }

    settings.store.save_index(&index)?;
    render_json(
        serde_json::json!({
            "domain": args.domain,
            "package": args.package,
            "source_domain": package.header.domain_id,
            "source_epoch_id": package.header.epoch_id,
            "summary_hash": hex_hash(package.header.summary_hash),
            "package_hash": hex_hash(package.package_hash),
            "results": results.iter().map(ImportRecord::json_summary).collect::<Vec<_>>(),
        }),
        args.json,
    );
    Ok(())
}

fn show_status(args: cli::StatusArgs) -> anyhow::Result<()> {
    let settings = ImporterSettings::load(args.domain, None, args.store_dir)?;
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
            "records": index.imports.iter().map(ImportRecord::json_summary).collect::<Vec<_>>(),
        })
    };
    render_json(output, args.json);
    Ok(())
}

fn show_record(args: cli::ShowArgs) -> anyhow::Result<()> {
    let settings = ImporterSettings::load(args.domain, None, args.store_dir)?;
    let export_id = parse_export_id_hex(&args.export_id)?;
    let record = settings
        .store
        .load_record(export_id)?
        .ok_or_else(|| anyhow!("no importer record found for export_id {}", args.export_id))?;
    render_json(record.json_summary(), args.json);
    Ok(())
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

fn verify_export_proofs(
    package: &CertifiedSummaryPackage,
    importer_domain: DomainId,
) -> anyhow::Result<Vec<ialp_common_types::ExportInclusionProof>> {
    if package.inclusion_proofs.len() < 2 {
        bail!("Phase 2B package must include at least one export proof");
    }

    let mut export_proofs = Vec::new();
    let mut package_target_domain = None;
    for encoded in &package.inclusion_proofs[1..] {
        let proof = match InclusionProof::decode(&mut &encoded[..])
            .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?
        {
            InclusionProof::ExportV1(proof) => proof,
            InclusionProof::SummaryHeaderStorageV1(_) => {
                bail!("summary-header proof may appear only at inclusion_proofs[0]")
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
        match package_target_domain {
            Some(target_domain) if target_domain != proof.leaf.target_domain => {
                bail!("package contains mixed target domains across export proofs")
            }
            None => package_target_domain = Some(proof.leaf.target_domain),
            _ => {}
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

fn decode_summary_storage_proof(
    bytes: &[u8],
) -> anyhow::Result<ialp_common_types::SummaryHeaderStorageProof> {
    match InclusionProof::decode(&mut &bytes[..])
        .map_err(|error| anyhow!("failed to decode summary proof: {error}"))?
    {
        InclusionProof::SummaryHeaderStorageV1(proof) => Ok(proof),
        InclusionProof::ExportV1(_) => bail!("expected summary-header storage proof at index 0"),
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

fn build_duplicate_record(
    package: &CertifiedSummaryPackage,
    proof: &ialp_common_types::ExportInclusionProof,
    duplicate_status: DuplicateStatus,
    submission_status: SubmissionStatus,
    reason: Option<String>,
) -> ImportRecord {
    ImportRecord {
        export_id: hex_hash(proof.leaf.export_id),
        source_domain: proof.leaf.source_domain,
        target_domain: proof.leaf.target_domain,
        summary_hash: hex_hash(package.header.summary_hash),
        package_hash: hex_hash(package.package_hash),
        verification_status: VerificationStatus::Verified,
        duplicate_status,
        submission_status,
        tx_hash: None,
        reason,
    }
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

struct ImporterSettings {
    node_url: String,
    store: Store,
}

impl ImporterSettings {
    fn load(
        domain: DomainId,
        node_url: Option<String>,
        store_dir: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let loaded = load_domain_config(domain, None)
            .with_context(|| format!("failed to load importer config for domain {domain}"))?;
        let node_url = node_url
            .unwrap_or_else(|| format!("ws://127.0.0.1:{}", loaded.config.network.rpc_port));
        let store_dir =
            store_dir.unwrap_or_else(|| PathBuf::from("var/importer").join(domain.as_str()));
        let store = Store::new(store_dir, domain)?;
        Ok(Self { node_url, store })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ialp_common_types::{
        export_merkle_root, ExportInclusionProof, ExportLeaf, ExportLeafHashInput,
        GrandpaFinalityCertificate, SummaryCertificationBundle, SummaryHeaderStorageProof,
        SUMMARY_HEADER_STORAGE_PROOF_VERSION,
    };
    use tempfile::tempdir;

    fn sample_leaf(target_domain: DomainId) -> ExportLeaf {
        ExportLeaf::from_hash_input(ExportLeafHashInput {
            version: 1,
            export_id: [9u8; 32],
            source_domain: DomainId::Earth,
            target_domain,
            sender: [1u8; 32],
            recipient: [2u8; 32],
            amount: 50,
            source_epoch_id: 3,
            source_block_height: 11,
            extrinsic_index: 0,
        })
    }

    fn sample_package(target_domain: DomainId) -> CertifiedSummaryPackage {
        let leaf = sample_leaf(target_domain);
        let root = export_merkle_root(DomainId::Earth, 3, 10, 12, core::slice::from_ref(&leaf));
        let header = ialp_common_types::EpochSummaryHeader {
            version: 1,
            domain_id: DomainId::Earth,
            epoch_id: 3,
            prev_summary_hash: [0u8; 32],
            start_block_height: 10,
            end_block_height: 12,
            state_root: [1u8; 32],
            block_root: [2u8; 32],
            tx_root: [3u8; 32],
            event_root: [4u8; 32],
            export_root: root,
            import_root: [5u8; 32],
            governance_root: [6u8; 32],
            validator_set_hash: [7u8; 32],
            summary_hash: [8u8; 32],
        };
        let bundle = SummaryCertificationBundle {
            certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                version: 1,
                grandpa_set_id: 0,
                target_block_number: 13,
                target_block_hash: [10u8; 32],
                proof_block_number: 13,
                proof_block_hash: [10u8; 32],
                justification: Vec::new(),
                ancestry_headers: Vec::new(),
            }),
            summary_header_storage_proof: SummaryHeaderStorageProof {
                version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                proof_block_number: 13,
                proof_block_hash: [10u8; 32],
                proof_block_header: Vec::new(),
                storage_key: summary_header_storage_key(3),
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
    fn rejects_packages_without_export_proofs() {
        let header = ialp_common_types::EpochSummaryHeader {
            version: 1,
            domain_id: DomainId::Earth,
            epoch_id: 3,
            prev_summary_hash: [0u8; 32],
            start_block_height: 10,
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
        let package = CertifiedSummaryPackage::from_bundle(
            header,
            SummaryCertificationBundle {
                certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                    version: 1,
                    grandpa_set_id: 0,
                    target_block_number: 13,
                    target_block_hash: [10u8; 32],
                    proof_block_number: 13,
                    proof_block_hash: [10u8; 32],
                    justification: Vec::new(),
                    ancestry_headers: Vec::new(),
                }),
                summary_header_storage_proof: SummaryHeaderStorageProof {
                    version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                    proof_block_number: 13,
                    proof_block_hash: [10u8; 32],
                    proof_block_header: Vec::new(),
                    storage_key: summary_header_storage_key(3),
                    trie_nodes: Vec::new(),
                },
            },
        );

        let error = verify_export_proofs(&package, DomainId::Moon)
            .expect_err("packages without export proofs should fail");
        assert!(error.to_string().contains("at least one export proof"));
    }

    #[test]
    fn rejects_packages_for_wrong_target_domain() {
        let error = verify_export_proofs(&sample_package(DomainId::Moon), DomainId::Mars)
            .expect_err("wrong target domain should fail");
        assert!(error.to_string().contains("package target domain"));
    }

    #[test]
    fn rejects_export_proofs_with_wrong_merkle_root() {
        let mut package = sample_package(DomainId::Moon);
        package.header.export_root = [0u8; 32];

        let error =
            verify_export_proofs(&package, DomainId::Moon).expect_err("wrong root should fail");
        assert!(error
            .to_string()
            .contains("export inclusion proof failed against header.export_root"));
    }

    #[test]
    fn rejects_mixed_target_domains_in_one_package() {
        let moon_leaf = sample_leaf(DomainId::Moon);
        let second_leaf = ExportLeaf::from_hash_input(ExportLeafHashInput {
            version: 1,
            export_id: [42u8; 32],
            source_domain: DomainId::Earth,
            target_domain: DomainId::Mars,
            sender: [1u8; 32],
            recipient: [2u8; 32],
            amount: 50,
            source_epoch_id: 3,
            source_block_height: 11,
            extrinsic_index: 1,
        });
        let root = export_merkle_root(
            DomainId::Earth,
            3,
            10,
            12,
            &[moon_leaf.clone(), second_leaf.clone()],
        );
        let mut package = sample_package(DomainId::Moon);
        package.header.export_root = root;
        package.inclusion_proofs = vec![package.inclusion_proofs[0].clone()];
        package.inclusion_proofs.push(
            InclusionProof::ExportV1(
                ialp_common_types::build_export_inclusion_proof(
                    &[moon_leaf.clone(), second_leaf.clone()],
                    moon_leaf.export_id,
                )
                .expect("proof"),
            )
            .encode(),
        );
        package.inclusion_proofs.push(
            InclusionProof::ExportV1(
                ialp_common_types::build_export_inclusion_proof(
                    &[moon_leaf, second_leaf],
                    [42u8; 32],
                )
                .expect("proof"),
            )
            .encode(),
        );

        let error = verify_export_proofs(&package, DomainId::Moon)
            .expect_err("mixed target domains should fail");
        assert!(error.to_string().contains("mixed target domains"));
    }

    #[test]
    fn package_hash_check_detects_drift() {
        let mut package = sample_package(DomainId::Moon);
        package.package_hash = [0u8; 32];
        assert!(verify_package_hash(&package).is_err());
    }

    #[test]
    fn importer_store_persists_duplicate_marker_by_export_id() {
        let root = tempdir().expect("tempdir");
        let store = Store::new(root.path().to_path_buf(), DomainId::Moon).expect("store");
        let mut index = store.load_index().expect("index");
        let package = sample_package(DomainId::Moon);
        let mut proofs = verify_export_proofs(&package, DomainId::Moon).expect("proofs");
        let proof = proofs.remove(0);
        let record = build_duplicate_record(
            &package,
            &proof,
            DuplicateStatus::DuplicateLocal,
            SubmissionStatus::SkippedDuplicate,
            Some("duplicate".into()),
        );

        store
            .persist_record(&mut index, record)
            .expect("record persisted");
        store.save_index(&index).expect("index saved");

        let loaded = store
            .load_record(proof.leaf.export_id)
            .expect("load")
            .expect("record exists");
        assert!(matches!(
            loaded.duplicate_status,
            DuplicateStatus::DuplicateLocal
        ));
    }
}
