use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context};
use codec::{Decode, Encode};
use ialp_common_types::{
    summary_header_storage_key, CertificationPendingReason, CertifiedSummaryPackage, DomainId,
    EpochId, InclusionProof, SummaryCertificate, SummaryHeaderStorageProof,
};
use serde::{Deserialize, Serialize};

const INDEX_SCHEMA_VERSION: u16 = 2;
const MANIFEST_SCHEMA_VERSION: u16 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExporterIndex {
    pub schema_version: u16,
    pub domain_id: DomainId,
    pub latest_staged_epoch: Option<EpochId>,
    pub latest_certified_epoch: Option<EpochId>,
    pub epochs: Vec<PackageRecord>,
}

impl ExporterIndex {
    pub fn new(domain_id: DomainId) -> Self {
        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            domain_id,
            latest_staged_epoch: None,
            latest_certified_epoch: None,
            epochs: Vec::new(),
        }
    }

    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "schema_version": self.schema_version,
            "domain_id": self.domain_id,
            "latest_staged_epoch": self.latest_staged_epoch,
            "latest_certified_epoch": self.latest_certified_epoch,
            "records": self.epochs.len(),
        })
    }

    pub fn latest_certified_record(&self) -> Option<&PackageRecord> {
        self.latest_certified_epoch
            .and_then(|epoch_id| self.record(epoch_id))
            .filter(|record| record.status == PackageStatus::Certified)
    }

    pub fn record(&self, epoch_id: EpochId) -> Option<&PackageRecord> {
        self.epochs
            .iter()
            .find(|record| record.epoch_id == epoch_id)
    }

    pub fn record_mut(&mut self, epoch_id: EpochId) -> Option<&mut PackageRecord> {
        self.epochs
            .iter_mut()
            .find(|record| record.epoch_id == epoch_id)
    }

    pub fn upsert_pending(
        &mut self,
        epoch_id: EpochId,
        summary_hash: [u8; 32],
        staged_at_block_number: u32,
        staged_at_block_hash: [u8; 32],
        pending_reason: Option<CertificationPendingReason>,
    ) {
        let summary_hash = hex_hash(summary_hash);
        let staged_at_block_hash = hex_hash(staged_at_block_hash);
        match self.record_mut(epoch_id) {
            Some(record) => {
                record.summary_hash = summary_hash;
                record.staged_at_block_number = staged_at_block_number;
                record.staged_at_block_hash = staged_at_block_hash;
                if record.status != PackageStatus::Certified {
                    record.status = PackageStatus::Staged;
                    record.pending_reason = pending_reason.map(|reason| format!("{reason:?}"));
                    record.package_hash = None;
                    record.package_scale_path = None;
                    record.package_manifest_path = None;
                    record.proof_block_number = None;
                    record.proof_block_hash = None;
                    record.proof_kind = None;
                    record.storage_key = None;
                    record.proof_node_count = None;
                    record.proof_total_bytes = None;
                    record.proof_block_header_len = None;
                }
            }
            None => self.epochs.push(PackageRecord {
                epoch_id,
                summary_hash,
                staged_at_block_number,
                staged_at_block_hash,
                status: PackageStatus::Staged,
                pending_reason: pending_reason.map(|reason| format!("{reason:?}")),
                package_hash: None,
                package_scale_path: None,
                package_manifest_path: None,
                proof_block_number: None,
                proof_block_hash: None,
                proof_kind: None,
                storage_key: None,
                proof_node_count: None,
                proof_total_bytes: None,
                proof_block_header_len: None,
            }),
        }
    }

    fn mark_certified(
        &mut self,
        epoch_id: EpochId,
        package_hash: [u8; 32],
        package_scale_path: &Path,
        package_manifest_path: &Path,
        proof_block_number: u32,
        proof_block_hash: [u8; 32],
        proof_metadata: &ProofMetadata,
    ) {
        let record = self
            .record_mut(epoch_id)
            .expect("pending record to exist before certification");
        record.status = PackageStatus::Certified;
        record.pending_reason = None;
        record.package_hash = Some(hex_hash(package_hash));
        record.package_scale_path = Some(package_scale_path.display().to_string());
        record.package_manifest_path = Some(package_manifest_path.display().to_string());
        record.proof_block_number = Some(proof_block_number);
        record.proof_block_hash = Some(hex_hash(proof_block_hash));
        record.proof_kind = Some(proof_metadata.kind.clone());
        record.storage_key = Some(hex_bytes(&proof_metadata.storage_key));
        record.proof_node_count = Some(proof_metadata.node_count);
        record.proof_total_bytes = Some(proof_metadata.total_bytes);
        record.proof_block_header_len = Some(proof_metadata.proof_block_header_len);
        self.latest_certified_epoch = Some(epoch_id);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageStatus {
    Staged,
    Certified,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageRecord {
    pub epoch_id: EpochId,
    pub summary_hash: String,
    pub staged_at_block_number: u32,
    pub staged_at_block_hash: String,
    pub status: PackageStatus,
    pub pending_reason: Option<String>,
    pub package_hash: Option<String>,
    pub package_scale_path: Option<String>,
    pub package_manifest_path: Option<String>,
    pub proof_block_number: Option<u32>,
    pub proof_block_hash: Option<String>,
    pub proof_kind: Option<String>,
    pub storage_key: Option<String>,
    pub proof_node_count: Option<usize>,
    pub proof_total_bytes: Option<usize>,
    pub proof_block_header_len: Option<usize>,
}

impl PackageRecord {
    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "epoch_id": self.epoch_id,
            "summary_hash": self.summary_hash,
            "staged_at_block_number": self.staged_at_block_number,
            "staged_at_block_hash": self.staged_at_block_hash,
            "status": self.status,
            "pending_reason": self.pending_reason,
            "package_hash": self.package_hash,
            "proof_block_number": self.proof_block_number,
            "proof_block_hash": self.proof_block_hash,
            "proof_kind": self.proof_kind,
            "storage_key": self.storage_key,
            "proof_node_count": self.proof_node_count,
            "proof_total_bytes": self.proof_total_bytes,
            "proof_block_header_len": self.proof_block_header_len,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageManifest {
    pub schema_version: u16,
    pub domain_id: DomainId,
    pub epoch_id: EpochId,
    pub summary_hash: String,
    pub package_hash: String,
    pub target_block_number: u32,
    pub target_block_hash: String,
    pub proof_block_number: u32,
    pub proof_block_hash: String,
    pub grandpa_set_id: u64,
    pub justification_len: usize,
    pub ancestry_header_count: usize,
    pub proof_kind: String,
    pub storage_key: String,
    pub proof_node_count: usize,
    pub proof_total_bytes: usize,
    pub proof_block_header_len: usize,
    pub inclusion_proofs_state: String,
    pub artifacts_state: String,
    pub scale_path: String,
}

#[derive(Clone, Debug)]
struct ProofMetadata {
    kind: String,
    storage_key: Vec<u8>,
    node_count: usize,
    total_bytes: usize,
    proof_block_header_len: usize,
}

impl ProofMetadata {
    fn from_summary_header_storage_proof(proof: &SummaryHeaderStorageProof) -> Self {
        Self {
            kind: "summary_header_storage_v1".into(),
            storage_key: proof.storage_key.clone(),
            node_count: proof.node_count(),
            total_bytes: proof.total_proof_bytes(),
            proof_block_header_len: proof.proof_block_header.len(),
        }
    }
}

pub struct Store {
    root: PathBuf,
    packages_dir: PathBuf,
    domain_id: DomainId,
}

impl Store {
    pub fn new(root: PathBuf, domain_id: DomainId) -> anyhow::Result<Self> {
        let packages_dir = root.join("packages");
        fs::create_dir_all(&packages_dir).with_context(|| {
            format!(
                "failed to create exporter package dir {}",
                packages_dir.display()
            )
        })?;
        Ok(Self {
            root,
            packages_dir,
            domain_id,
        })
    }

    pub fn load_index(&self) -> anyhow::Result<ExporterIndex> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(ExporterIndex::new(self.domain_id));
        }
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read exporter index {}", path.display()))?;
        let raw: serde_json::Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to decode exporter index {}", path.display()))?;
        let schema_version = raw
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u16;

        let index = match schema_version {
            1 => {
                let legacy: LegacyExporterIndexV1 = serde_json::from_value(raw).with_context(|| {
                    format!("failed to decode legacy exporter index {}", path.display())
                })?;
                legacy.into_current()
            }
            INDEX_SCHEMA_VERSION => serde_json::from_value(raw).with_context(|| {
                format!("failed to decode exporter index {}", path.display())
            })?,
            other => bail!(
                "unsupported exporter index schema version {other}; expected 1 or {INDEX_SCHEMA_VERSION}"
            ),
        };

        if index.domain_id != self.domain_id {
            bail!(
                "exporter index domain {} does not match requested domain {}",
                index.domain_id,
                self.domain_id
            );
        }

        Ok(index)
    }

    pub fn save_index(&self, index: &ExporterIndex) -> anyhow::Result<()> {
        self.ensure_index_domain(index)?;
        let bytes =
            serde_json::to_vec_pretty(index).context("failed to serialize exporter index")?;
        self.atomic_replace(&self.index_path(), &bytes)
    }

    pub fn persist_package(
        &self,
        index: &mut ExporterIndex,
        package: &CertifiedSummaryPackage,
    ) -> anyhow::Result<()> {
        self.ensure_index_domain(index)?;
        let epoch_id = package.header.epoch_id;
        let summary_hash = hex_hash(package.header.summary_hash);

        if let Some(existing) = index.record(epoch_id) {
            if existing.summary_hash != summary_hash {
                bail!(
                    "epoch {epoch_id} already has summary hash {} recorded; refusing to overwrite with {}",
                    existing.summary_hash,
                    summary_hash
                );
            }
        }

        let scale_path = self.package_scale_path(epoch_id);
        let manifest_path = self.package_manifest_path(epoch_id);
        self.archive_legacy_package_files_if_needed(&scale_path, &manifest_path)?;
        let scale_bytes = package.encode();
        let (manifest, proof_metadata) = self.build_manifest(package, &scale_path)?;
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).context("failed to serialize package manifest")?;

        self.atomic_write(&scale_path, &scale_bytes)?;
        self.atomic_write(&manifest_path, &manifest_bytes)?;

        let SummaryCertificate::GrandpaV1(certificate) = &package.certificate;
        index.mark_certified(
            epoch_id,
            package.package_hash,
            &scale_path,
            &manifest_path,
            certificate.proof_block_number,
            certificate.proof_block_hash,
            &proof_metadata,
        );
        Ok(())
    }

    pub fn load_package(
        &self,
        epoch_id: EpochId,
    ) -> anyhow::Result<(PackageManifest, CertifiedSummaryPackage)> {
        let manifest_path = self.package_manifest_path(epoch_id);
        let scale_path = self.package_scale_path(epoch_id);

        let manifest_bytes = fs::read(&manifest_path).with_context(|| {
            format!(
                "failed to read package manifest {}",
                manifest_path.display()
            )
        })?;
        let raw_manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
            .with_context(|| {
                format!(
                    "failed to decode package manifest {}",
                    manifest_path.display()
                )
            })?;
        let manifest_schema = raw_manifest
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u16;
        if manifest_schema != MANIFEST_SCHEMA_VERSION {
            bail!(
                "package manifest schema {} is not Phase 2A proof-bearing schema {}",
                manifest_schema,
                MANIFEST_SCHEMA_VERSION
            );
        }
        let manifest: PackageManifest =
            serde_json::from_value(raw_manifest).with_context(|| {
                format!(
                    "failed to decode package manifest {}",
                    manifest_path.display()
                )
            })?;

        let scale_bytes = fs::read(&scale_path).with_context(|| {
            format!("failed to read package scale file {}", scale_path.display())
        })?;
        let package = CertifiedSummaryPackage::decode(&mut &scale_bytes[..])
            .map_err(|error| anyhow!("failed to decode certified summary package: {error}"))?;

        let computed_hash = hex_hash(package.compute_package_hash());
        if computed_hash != manifest.package_hash {
            bail!(
                "package hash mismatch for epoch {epoch_id}: manifest={}, computed={computed_hash}",
                manifest.package_hash
            );
        }

        Ok((manifest, package))
    }

    fn ensure_index_domain(&self, index: &ExporterIndex) -> anyhow::Result<()> {
        if index.domain_id != self.domain_id {
            bail!(
                "exporter index domain {} does not match store domain {}",
                index.domain_id,
                self.domain_id
            );
        }
        Ok(())
    }

    fn build_manifest(
        &self,
        package: &CertifiedSummaryPackage,
        scale_path: &Path,
    ) -> anyhow::Result<(PackageManifest, ProofMetadata)> {
        let SummaryCertificate::GrandpaV1(certificate) = &package.certificate;
        if package.inclusion_proofs.len() != 1 {
            bail!(
                "Phase 2A package manifest requires exactly one inclusion proof, found {}",
                package.inclusion_proofs.len()
            );
        }
        if !package.artifacts.is_empty() {
            bail!("Phase 2A package manifest only supports empty artifacts");
        }

        let proof = decode_summary_header_storage_proof(&package.inclusion_proofs[0])?;
        if proof.proof_block_number != certificate.proof_block_number
            || proof.proof_block_hash != certificate.proof_block_hash
        {
            bail!("summary header storage proof block reference does not match certificate");
        }

        let expected_key = summary_header_storage_key(package.header.epoch_id);
        if proof.storage_key != expected_key {
            bail!("summary header storage proof does not target canonical SummaryHeaders storage");
        }

        let proof_metadata = ProofMetadata::from_summary_header_storage_proof(&proof);
        Ok((
            PackageManifest {
                schema_version: MANIFEST_SCHEMA_VERSION,
                domain_id: package.header.domain_id,
                epoch_id: package.header.epoch_id,
                summary_hash: hex_hash(package.header.summary_hash),
                package_hash: hex_hash(package.package_hash),
                target_block_number: certificate.target_block_number,
                target_block_hash: hex_hash(certificate.target_block_hash),
                proof_block_number: certificate.proof_block_number,
                proof_block_hash: hex_hash(certificate.proof_block_hash),
                grandpa_set_id: certificate.grandpa_set_id,
                justification_len: certificate.justification.len(),
                ancestry_header_count: certificate.ancestry_headers.len(),
                proof_kind: proof_metadata.kind.clone(),
                storage_key: hex_bytes(&proof_metadata.storage_key),
                proof_node_count: proof_metadata.node_count,
                proof_total_bytes: proof_metadata.total_bytes,
                proof_block_header_len: proof_metadata.proof_block_header_len,
                inclusion_proofs_state: "summary_header_storage_v1_only".into(),
                artifacts_state: "empty_phase_2a".into(),
                scale_path: scale_path.display().to_string(),
            },
            proof_metadata,
        ))
    }

    fn archive_legacy_package_files_if_needed(
        &self,
        scale_path: &Path,
        manifest_path: &Path,
    ) -> anyhow::Result<()> {
        if !manifest_path.exists() {
            return Ok(());
        }

        let manifest_bytes = fs::read(manifest_path).with_context(|| {
            format!(
                "failed to read existing package manifest {}",
                manifest_path.display()
            )
        })?;
        let raw_manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
            .with_context(|| {
                format!(
                    "failed to decode existing package manifest {}",
                    manifest_path.display()
                )
            })?;
        let manifest_schema = raw_manifest
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u16;

        if manifest_schema != 1 {
            return Ok(());
        }

        self.archive_legacy_file(manifest_path)?;
        if scale_path.exists() {
            self.archive_legacy_file(scale_path)?;
        }
        Ok(())
    }

    fn archive_legacy_file(&self, path: &Path) -> anyhow::Result<()> {
        let archived = path.with_extension(format!(
            "{}.schema1.legacy",
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("artifact")
        ));
        if archived.exists() {
            fs::remove_file(&archived).with_context(|| {
                format!(
                    "failed to remove previous archived legacy file {}",
                    archived.display()
                )
            })?;
        }
        fs::rename(path, &archived).with_context(|| {
            format!(
                "failed to archive legacy Phase 1B file {} to {}",
                path.display(),
                archived.display()
            )
        })?;
        Ok(())
    }

    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        if path.exists() {
            let existing = fs::read(path)
                .with_context(|| format!("failed to read existing file {}", path.display()))?;
            if existing == bytes {
                return Ok(());
            }

            bail!(
                "refusing to overwrite existing file {} with different bytes",
                path.display()
            );
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create parent dir {}", parent.display()))?;
        }

        let tmp_path = path.with_extension(format!(
            "{}.tmp",
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("write")
        ));
        fs::write(&tmp_path, bytes)
            .with_context(|| format!("failed to write temporary file {}", tmp_path.display()))?;
        fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "failed to atomically move {} to {}",
                tmp_path.display(),
                path.display()
            )
        })?;
        Ok(())
    }

    fn atomic_replace(&self, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create parent dir {}", parent.display()))?;
        }

        let tmp_path = path.with_extension(format!(
            "{}.tmp",
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("write")
        ));
        fs::write(&tmp_path, bytes)
            .with_context(|| format!("failed to write temporary file {}", tmp_path.display()))?;
        fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "failed to atomically replace {} with {}",
                path.display(),
                tmp_path.display()
            )
        })?;
        Ok(())
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn package_scale_path(&self, epoch_id: EpochId) -> PathBuf {
        self.packages_dir.join(format!("epoch-{epoch_id}.scale"))
    }

    fn package_manifest_path(&self, epoch_id: EpochId) -> PathBuf {
        self.packages_dir.join(format!("epoch-{epoch_id}.json"))
    }
}

#[derive(Clone, Debug, Deserialize)]
struct LegacyExporterIndexV1 {
    schema_version: u16,
    domain_id: DomainId,
    latest_staged_epoch: Option<EpochId>,
    latest_certified_epoch: Option<EpochId>,
    epochs: Vec<LegacyPackageRecordV1>,
}

impl LegacyExporterIndexV1 {
    fn into_current(self) -> ExporterIndex {
        debug_assert_eq!(self.schema_version, 1);
        let _legacy_latest_certified_epoch = self.latest_certified_epoch;
        ExporterIndex {
            schema_version: INDEX_SCHEMA_VERSION,
            domain_id: self.domain_id,
            latest_staged_epoch: self.latest_staged_epoch,
            latest_certified_epoch: None,
            epochs: self
                .epochs
                .into_iter()
                .map(LegacyPackageRecordV1::into_current)
                .collect(),
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
struct LegacyPackageRecordV1 {
    epoch_id: EpochId,
    summary_hash: String,
    staged_at_block_number: u32,
    staged_at_block_hash: String,
    status: PackageStatus,
    pending_reason: Option<String>,
    package_hash: Option<String>,
    package_scale_path: Option<String>,
    package_manifest_path: Option<String>,
    proof_block_number: Option<u32>,
    proof_block_hash: Option<String>,
}

impl LegacyPackageRecordV1 {
    fn into_current(self) -> PackageRecord {
        let was_certified = self.status == PackageStatus::Certified;
        PackageRecord {
            epoch_id: self.epoch_id,
            summary_hash: self.summary_hash,
            staged_at_block_number: self.staged_at_block_number,
            staged_at_block_hash: self.staged_at_block_hash,
            status: PackageStatus::Staged,
            pending_reason: if was_certified {
                None
            } else {
                self.pending_reason
            },
            package_hash: None,
            package_scale_path: None,
            package_manifest_path: None,
            proof_block_number: None,
            proof_block_hash: None,
            proof_kind: None,
            storage_key: None,
            proof_node_count: None,
            proof_total_bytes: None,
            proof_block_header_len: None,
        }
    }
}

fn decode_summary_header_storage_proof(bytes: &[u8]) -> anyhow::Result<SummaryHeaderStorageProof> {
    let proof = InclusionProof::decode(&mut &bytes[..])
        .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?;
    match proof {
        InclusionProof::SummaryHeaderStorageV1(proof) => Ok(proof),
    }
}

fn hex_hash(hash: [u8; 32]) -> String {
    format!("0x{}", hex::encode(hash))
}

fn hex_bytes(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ialp_common_types::{
        CertifiedSummaryPackage, GrandpaFinalityCertificate, SummaryCertificate,
        SummaryCertificationBundle, SUMMARY_HEADER_STORAGE_PROOF_VERSION,
    };

    fn temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "ialp-exporter-test-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should work")
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    fn sample_package() -> CertifiedSummaryPackage {
        let header = ialp_common_types::EpochSummaryHeader {
            version: 1,
            domain_id: DomainId::Earth,
            epoch_id: 2,
            prev_summary_hash: [1u8; 32],
            start_block_height: 7,
            end_block_height: 9,
            state_root: [2u8; 32],
            block_root: [3u8; 32],
            tx_root: [4u8; 32],
            event_root: [5u8; 32],
            export_root: [6u8; 32],
            import_root: [7u8; 32],
            governance_root: [8u8; 32],
            validator_set_hash: [9u8; 32],
            summary_hash: [10u8; 32],
        };

        CertifiedSummaryPackage::from_bundle(
            header,
            SummaryCertificationBundle {
                certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                    version: 1,
                    grandpa_set_id: 0,
                    target_block_number: 10,
                    target_block_hash: [11u8; 32],
                    proof_block_number: 11,
                    proof_block_hash: [12u8; 32],
                    justification: vec![1, 2, 3, 4],
                    ancestry_headers: vec![vec![9, 9]],
                }),
                summary_header_storage_proof: SummaryHeaderStorageProof {
                    version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                    proof_block_number: 11,
                    proof_block_hash: [12u8; 32],
                    proof_block_header: vec![0xaa, 0xbb, 0xcc],
                    storage_key: summary_header_storage_key(2),
                    trie_nodes: vec![vec![1, 2, 3], vec![4, 5]],
                },
            },
        )
    }

    #[test]
    fn persisting_same_package_twice_is_idempotent() {
        let root = temp_dir("idempotent");
        let store = Store::new(root, DomainId::Earth).expect("store should build");
        let mut index = ExporterIndex::new(DomainId::Earth);
        let package = sample_package();

        index.upsert_pending(
            package.header.epoch_id,
            package.header.summary_hash,
            10,
            [11u8; 32],
            None,
        );
        store
            .persist_package(&mut index, &package)
            .expect("first persist should succeed");
        store
            .persist_package(&mut index, &package)
            .expect("second identical persist should succeed");
    }

    #[test]
    fn loading_package_recomputes_hash() {
        let root = temp_dir("load");
        let store = Store::new(root, DomainId::Earth).expect("store should build");
        let mut index = ExporterIndex::new(DomainId::Earth);
        let package = sample_package();

        index.upsert_pending(
            package.header.epoch_id,
            package.header.summary_hash,
            10,
            [11u8; 32],
            None,
        );
        store
            .persist_package(&mut index, &package)
            .expect("persist should succeed");

        let (_, loaded) = store
            .load_package(package.header.epoch_id)
            .expect("stored package should load");
        assert_eq!(loaded.package_hash, package.package_hash);
    }

    #[test]
    fn refusing_to_overwrite_existing_package_with_different_bytes() {
        let root = temp_dir("overwrite");
        let store = Store::new(root, DomainId::Earth).expect("store should build");
        let mut index = ExporterIndex::new(DomainId::Earth);
        let package = sample_package();

        index.upsert_pending(
            package.header.epoch_id,
            package.header.summary_hash,
            10,
            [11u8; 32],
            None,
        );
        store
            .persist_package(&mut index, &package)
            .expect("persist should succeed");

        let mut different = sample_package();
        different.package_hash = [99u8; 32];
        let error = store
            .persist_package(&mut index, &different)
            .expect_err("different bytes should be rejected");

        assert!(error
            .to_string()
            .contains("refusing to overwrite existing file"));
    }

    #[test]
    fn index_updates_can_replace_previous_bytes() {
        let root = temp_dir("index-replace");
        let store = Store::new(root, DomainId::Earth).expect("store should build");
        let mut index = ExporterIndex::new(DomainId::Earth);

        store
            .save_index(&index)
            .expect("initial index write should succeed");
        index.latest_staged_epoch = Some(4);
        store
            .save_index(&index)
            .expect("updated index write should replace previous bytes");
    }

    #[test]
    fn manifest_records_phase_2a_proof_metadata() {
        let root = temp_dir("manifest");
        let store = Store::new(root, DomainId::Earth).expect("store should build");
        let mut index = ExporterIndex::new(DomainId::Earth);
        let package = sample_package();

        index.upsert_pending(
            package.header.epoch_id,
            package.header.summary_hash,
            10,
            [11u8; 32],
            None,
        );
        store
            .persist_package(&mut index, &package)
            .expect("persist should succeed");

        let (manifest, _) = store
            .load_package(package.header.epoch_id)
            .expect("package should load");
        assert_eq!(manifest.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(manifest.proof_kind, "summary_header_storage_v1");
        assert_eq!(
            manifest.storage_key,
            hex_bytes(&summary_header_storage_key(2))
        );
        assert_eq!(manifest.proof_node_count, 2);
        assert_eq!(manifest.artifacts_state, "empty_phase_2a");
    }

    #[test]
    fn load_index_migrates_schema_one_records_back_to_staged() {
        let root = temp_dir("legacy-index");
        let store = Store::new(root.clone(), DomainId::Earth).expect("store should build");
        let legacy = serde_json::json!({
            "schema_version": 1,
            "domain_id": "earth",
            "latest_staged_epoch": 4,
            "latest_certified_epoch": 4,
            "epochs": [{
                "epoch_id": 4,
                "summary_hash": "0x11",
                "staged_at_block_number": 22,
                "staged_at_block_hash": "0x22",
                "status": "certified",
                "pending_reason": null,
                "package_hash": "0x33",
                "package_scale_path": "/tmp/old.scale",
                "package_manifest_path": "/tmp/old.json",
                "proof_block_number": 23,
                "proof_block_hash": "0x44"
            }]
        });
        fs::write(
            root.join("index.json"),
            serde_json::to_vec_pretty(&legacy).expect("legacy json"),
        )
        .expect("legacy index write");

        let migrated = store.load_index().expect("legacy index should migrate");
        let record = migrated.record(4).expect("migrated record");

        assert_eq!(migrated.schema_version, INDEX_SCHEMA_VERSION);
        assert_eq!(migrated.latest_staged_epoch, Some(4));
        assert_eq!(migrated.latest_certified_epoch, None);
        assert_eq!(record.status, PackageStatus::Staged);
        assert!(record.package_hash.is_none());
        assert!(record.proof_kind.is_none());
    }
}
