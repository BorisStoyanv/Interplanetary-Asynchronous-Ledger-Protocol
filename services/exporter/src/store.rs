use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context};
use codec::{Decode, Encode};
use ialp_common_types::{
    CertifiedSummaryPackage, DomainId, EpochId, ExportId, ExportInclusionProof,
    FinalizedImportInclusionProof, GovernanceInclusionProof, InclusionProof, SummaryCertificate,
    SummaryHeaderStorageProof,
};
use serde::{Deserialize, Serialize};

const INDEX_SCHEMA_VERSION: u16 = 4;
const MANIFEST_SCHEMA_VERSION: u16 = 3;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExporterIndex {
    pub schema_version: u16,
    pub domain_id: DomainId,
    pub latest_staged_epoch: Option<EpochId>,
    pub latest_certified_epoch: Option<EpochId>,
    pub latest_certified_target_domain: Option<DomainId>,
    pub packages: Vec<PackageRecord>,
}

impl ExporterIndex {
    pub fn new(domain_id: DomainId) -> Self {
        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            domain_id,
            latest_staged_epoch: None,
            latest_certified_epoch: None,
            latest_certified_target_domain: None,
            packages: Vec::new(),
        }
    }

    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "schema_version": self.schema_version,
            "domain_id": self.domain_id,
            "latest_staged_epoch": self.latest_staged_epoch,
            "latest_certified_epoch": self.latest_certified_epoch,
            "latest_certified_target_domain": self.latest_certified_target_domain,
            "package_records": self.packages.len(),
        })
    }

    pub fn latest_certified_record(&self) -> Option<&PackageRecord> {
        let epoch = self.latest_certified_epoch?;
        let target_domain = self.latest_certified_target_domain?;
        self.record(epoch, target_domain)
    }

    pub fn record(&self, epoch_id: EpochId, target_domain: DomainId) -> Option<&PackageRecord> {
        self.packages
            .iter()
            .find(|record| record.epoch_id == epoch_id && record.target_domain == target_domain)
    }

    pub fn records_for_epoch(&self, epoch_id: EpochId) -> Vec<&PackageRecord> {
        self.packages
            .iter()
            .filter(|record| record.epoch_id == epoch_id)
            .collect()
    }

    fn record_mut(
        &mut self,
        epoch_id: EpochId,
        target_domain: DomainId,
    ) -> Option<&mut PackageRecord> {
        self.packages
            .iter_mut()
            .find(|record| record.epoch_id == epoch_id && record.target_domain == target_domain)
    }

    pub fn upsert_pending(
        &mut self,
        epoch_id: EpochId,
        target_domain: DomainId,
        summary_hash: [u8; 32],
        staged_at_block_number: u32,
        staged_at_block_hash: [u8; 32],
        export_ids: &[ExportId],
        pending_reason: Option<String>,
    ) {
        let record = self.record_mut(epoch_id, target_domain);
        let export_ids = export_ids
            .iter()
            .map(|hash| hex_hash(*hash))
            .collect::<Vec<_>>();
        match record {
            Some(record) => {
                record.summary_hash = hex_hash(summary_hash);
                record.staged_at_block_number = staged_at_block_number;
                record.staged_at_block_hash = hex_hash(staged_at_block_hash);
                record.export_ids = export_ids;
                record.export_count = record.export_ids.len();
                if record.status != PackageStatus::Certified {
                    record.status = PackageStatus::Staged;
                    record.pending_reason = pending_reason;
                    record.package_hash = None;
                    record.package_scale_path = None;
                    record.package_manifest_path = None;
                    record.proof_block_number = None;
                    record.proof_block_hash = None;
                    record.proof_count = None;
                    record.proof_kinds = Vec::new();
                    record.relay_submission_state = RelaySubmissionState::NotSubmitted;
                    record.relay_last_submitted_at_unix_ms = None;
                    record.relay_submission_attempts = 0;
                    record.relay_last_error = None;
                }
            }
            None => {
                let export_count = export_ids.len();
                self.packages.push(PackageRecord {
                    epoch_id,
                    target_domain,
                    summary_hash: hex_hash(summary_hash),
                    staged_at_block_number,
                    staged_at_block_hash: hex_hash(staged_at_block_hash),
                    export_ids,
                    export_count,
                    status: PackageStatus::Staged,
                    pending_reason,
                    package_hash: None,
                    package_scale_path: None,
                    package_manifest_path: None,
                    proof_block_number: None,
                    proof_block_hash: None,
                    proof_count: None,
                    proof_kinds: Vec::new(),
                    relay_submission_state: RelaySubmissionState::NotSubmitted,
                    relay_last_submitted_at_unix_ms: None,
                    relay_submission_attempts: 0,
                    relay_last_error: None,
                })
            }
        }
    }

    fn mark_certified(
        &mut self,
        epoch_id: EpochId,
        target_domain: DomainId,
        package_hash: [u8; 32],
        package_scale_path: &Path,
        package_manifest_path: &Path,
        proof_block_number: u32,
        proof_block_hash: [u8; 32],
        proof_count: usize,
        proof_kinds: Vec<String>,
    ) {
        let record = self
            .record_mut(epoch_id, target_domain)
            .expect("pending package record should exist before certification");
        let was_certified = record.status == PackageStatus::Certified;
        record.status = PackageStatus::Certified;
        record.pending_reason = None;
        record.package_hash = Some(hex_hash(package_hash));
        record.package_scale_path = Some(package_scale_path.display().to_string());
        record.package_manifest_path = Some(package_manifest_path.display().to_string());
        record.proof_block_number = Some(proof_block_number);
        record.proof_block_hash = Some(hex_hash(proof_block_hash));
        record.proof_count = Some(proof_count);
        record.proof_kinds = proof_kinds;
        if !was_certified {
            record.relay_submission_state = RelaySubmissionState::NotSubmitted;
            record.relay_last_submitted_at_unix_ms = None;
            record.relay_submission_attempts = 0;
            record.relay_last_error = None;
        }
        self.latest_certified_epoch = Some(epoch_id);
        self.latest_certified_target_domain = Some(target_domain);
    }

    pub fn mark_relay_submitted(
        &mut self,
        epoch_id: EpochId,
        target_domain: DomainId,
        submitted_at_unix_ms: u64,
    ) {
        if let Some(record) = self.record_mut(epoch_id, target_domain) {
            record.relay_submission_state = RelaySubmissionState::Submitted;
            record.relay_last_submitted_at_unix_ms = Some(submitted_at_unix_ms);
            record.relay_submission_attempts = record.relay_submission_attempts.saturating_add(1);
            record.relay_last_error = None;
        }
    }

    pub fn mark_relay_submission_error(
        &mut self,
        epoch_id: EpochId,
        target_domain: DomainId,
        submitted_at_unix_ms: u64,
        retryable: bool,
        error: String,
    ) {
        if let Some(record) = self.record_mut(epoch_id, target_domain) {
            record.relay_submission_state = if retryable {
                RelaySubmissionState::SubmissionRetrying
            } else {
                RelaySubmissionState::SubmissionFailed
            };
            record.relay_last_submitted_at_unix_ms = Some(submitted_at_unix_ms);
            record.relay_submission_attempts = record.relay_submission_attempts.saturating_add(1);
            record.relay_last_error = Some(error);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageStatus {
    Staged,
    Certified,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelaySubmissionState {
    NotSubmitted,
    Submitted,
    SubmissionRetrying,
    SubmissionFailed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageRecord {
    pub epoch_id: EpochId,
    pub target_domain: DomainId,
    pub summary_hash: String,
    pub staged_at_block_number: u32,
    pub staged_at_block_hash: String,
    pub export_ids: Vec<String>,
    pub export_count: usize,
    pub status: PackageStatus,
    pub pending_reason: Option<String>,
    pub package_hash: Option<String>,
    pub package_scale_path: Option<String>,
    pub package_manifest_path: Option<String>,
    pub proof_block_number: Option<u32>,
    pub proof_block_hash: Option<String>,
    pub proof_count: Option<usize>,
    pub proof_kinds: Vec<String>,
    pub relay_submission_state: RelaySubmissionState,
    pub relay_last_submitted_at_unix_ms: Option<u64>,
    pub relay_submission_attempts: u32,
    pub relay_last_error: Option<String>,
}

impl PackageRecord {
    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "epoch_id": self.epoch_id,
            "target_domain": self.target_domain,
            "summary_hash": self.summary_hash,
            "staged_at_block_number": self.staged_at_block_number,
            "staged_at_block_hash": self.staged_at_block_hash,
            "export_ids": self.export_ids,
            "export_count": self.export_count,
            "status": self.status,
            "pending_reason": self.pending_reason,
            "package_hash": self.package_hash,
            "proof_block_number": self.proof_block_number,
            "proof_block_hash": self.proof_block_hash,
            "proof_count": self.proof_count,
            "proof_kinds": self.proof_kinds,
            "relay_submission_state": self.relay_submission_state,
            "relay_last_submitted_at_unix_ms": self.relay_last_submitted_at_unix_ms,
            "relay_submission_attempts": self.relay_submission_attempts,
            "relay_last_error": self.relay_last_error,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageManifest {
    pub schema_version: u16,
    pub domain_id: DomainId,
    pub target_domain: DomainId,
    pub epoch_id: EpochId,
    pub summary_hash: String,
    pub package_hash: String,
    pub export_ids: Vec<String>,
    pub export_count: usize,
    pub proof_count: usize,
    pub proof_kinds: Vec<String>,
    pub target_block_number: u32,
    pub target_block_hash: String,
    pub proof_block_number: u32,
    pub proof_block_hash: String,
    pub grandpa_set_id: u64,
    pub justification_len: usize,
    pub ancestry_header_count: usize,
    pub summary_storage_key: String,
    pub summary_storage_node_count: usize,
    pub summary_storage_total_bytes: usize,
    pub summary_storage_header_len: usize,
    pub artifacts_state: String,
    pub scale_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ExporterIndexV3 {
    pub schema_version: u16,
    pub domain_id: DomainId,
    pub latest_staged_epoch: Option<EpochId>,
    pub latest_certified_epoch: Option<EpochId>,
    pub latest_certified_target_domain: Option<DomainId>,
    pub packages: Vec<PackageRecordV3>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PackageRecordV3 {
    pub epoch_id: EpochId,
    pub target_domain: DomainId,
    pub summary_hash: String,
    pub staged_at_block_number: u32,
    pub staged_at_block_hash: String,
    pub export_ids: Vec<String>,
    pub export_count: usize,
    pub status: PackageStatus,
    pub pending_reason: Option<String>,
    pub package_hash: Option<String>,
    pub package_scale_path: Option<String>,
    pub package_manifest_path: Option<String>,
    pub proof_block_number: Option<u32>,
    pub proof_block_hash: Option<String>,
    pub proof_count: Option<usize>,
    pub proof_kinds: Vec<String>,
}

pub struct Store {
    root: PathBuf,
    packages_dir: PathBuf,
    legacy_dir: PathBuf,
    domain_id: DomainId,
}

impl Store {
    pub fn new(root: PathBuf, domain_id: DomainId) -> anyhow::Result<Self> {
        let packages_dir = root.join("packages");
        let legacy_dir = root.join("legacy");
        fs::create_dir_all(&packages_dir).with_context(|| {
            format!(
                "failed to create exporter package dir {}",
                packages_dir.display()
            )
        })?;
        fs::create_dir_all(&legacy_dir).with_context(|| {
            format!(
                "failed to create exporter legacy dir {}",
                legacy_dir.display()
            )
        })?;
        Ok(Self {
            root,
            packages_dir,
            legacy_dir,
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
            INDEX_SCHEMA_VERSION => serde_json::from_value(raw).with_context(|| {
                format!("failed to decode exporter index {}", path.display())
            })?,
            1..=3 => self.migrate_legacy_index(raw, schema_version)?,
            other => bail!(
                "unsupported exporter index schema version {other}; expected 1, 2, 3, or {INDEX_SCHEMA_VERSION}"
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
        if index.domain_id != self.domain_id {
            bail!(
                "exporter index domain {} does not match store domain {}",
                index.domain_id,
                self.domain_id
            );
        }
        let bytes =
            serde_json::to_vec_pretty(index).context("failed to serialize exporter index")?;
        self.atomic_write(&self.index_path(), &bytes)
    }

    pub fn persist_package(
        &self,
        index: &mut ExporterIndex,
        target_domain: DomainId,
        export_ids: &[ExportId],
        package: &CertifiedSummaryPackage,
    ) -> anyhow::Result<()> {
        let epoch_id = package.header.epoch_id;
        let summary_hash = hex_hash(package.header.summary_hash);
        if let Some(existing) = index.record(epoch_id, target_domain) {
            if existing.summary_hash != summary_hash {
                bail!(
                    "package key ({epoch_id}, {target_domain}) already recorded for summary {}",
                    existing.summary_hash
                );
            }
        }

        let scale_path = self.package_scale_path(epoch_id, target_domain);
        let manifest_path = self.package_manifest_path(epoch_id, target_domain);
        let scale_bytes = package.encode();

        if scale_path.exists() {
            let current = fs::read(&scale_path)
                .with_context(|| format!("failed to read {}", scale_path.display()))?;
            if current != scale_bytes {
                bail!(
                    "refusing to overwrite {} with different bytes for the same package identity",
                    scale_path.display()
                );
            }
        }

        self.archive_phase_2a_package_files_if_needed(epoch_id)?;

        let manifest = self.build_manifest(package, target_domain, export_ids, &scale_path)?;
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).context("failed to serialize manifest")?;

        self.atomic_write(&scale_path, &scale_bytes)?;
        self.atomic_write(&manifest_path, &manifest_bytes)?;

        let SummaryCertificate::GrandpaV1(certificate) = &package.certificate;
        index.mark_certified(
            epoch_id,
            target_domain,
            package.package_hash,
            &scale_path,
            &manifest_path,
            certificate.proof_block_number,
            certificate.proof_block_hash,
            package.inclusion_proofs.len(),
            manifest.proof_kinds.clone(),
        );
        Ok(())
    }

    pub fn load_package(
        &self,
        epoch_id: EpochId,
        target_domain: DomainId,
    ) -> anyhow::Result<(PackageManifest, CertifiedSummaryPackage)> {
        let manifest_path = self.package_manifest_path(epoch_id, target_domain);
        let scale_path = self.package_scale_path(epoch_id, target_domain);
        let manifest_bytes = fs::read(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        let scale_bytes = fs::read(&scale_path)
            .with_context(|| format!("failed to read {}", scale_path.display()))?;

        let manifest: PackageManifest = serde_json::from_slice(&manifest_bytes)
            .with_context(|| format!("failed to decode {}", manifest_path.display()))?;
        let package = CertifiedSummaryPackage::decode(&mut &scale_bytes[..])
            .map_err(|error| anyhow!("failed to decode package bytes: {error}"))?;
        Ok((manifest, package))
    }

    fn migrate_legacy_index(
        &self,
        raw: serde_json::Value,
        legacy_schema_version: u16,
    ) -> anyhow::Result<ExporterIndex> {
        let domain_id: DomainId = serde_json::from_value(
            raw.get("domain_id")
                .cloned()
                .ok_or_else(|| anyhow!("legacy exporter index is missing domain_id"))?,
        )
        .context("failed to decode legacy domain_id")?;
        let latest_staged_epoch = raw
            .get("latest_staged_epoch")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as EpochId);

        let archived_name = format!("index-schema-{legacy_schema_version}.json");
        self.atomic_write(
            &self.legacy_dir.join(archived_name),
            &serde_json::to_vec_pretty(&raw)?,
        )?;

        if legacy_schema_version == 3 {
            let legacy: ExporterIndexV3 =
                serde_json::from_value(raw).context("failed to decode schema 3 exporter index")?;
            let mut index = ExporterIndex::new(legacy.domain_id);
            index.latest_staged_epoch = legacy.latest_staged_epoch;
            index.latest_certified_epoch = legacy.latest_certified_epoch;
            index.latest_certified_target_domain = legacy.latest_certified_target_domain;
            index.packages = legacy
                .packages
                .into_iter()
                .map(|record| PackageRecord {
                    epoch_id: record.epoch_id,
                    target_domain: record.target_domain,
                    summary_hash: record.summary_hash,
                    staged_at_block_number: record.staged_at_block_number,
                    staged_at_block_hash: record.staged_at_block_hash,
                    export_ids: record.export_ids,
                    export_count: record.export_count,
                    status: record.status,
                    pending_reason: record.pending_reason,
                    package_hash: record.package_hash,
                    package_scale_path: record.package_scale_path,
                    package_manifest_path: record.package_manifest_path,
                    proof_block_number: record.proof_block_number,
                    proof_block_hash: record.proof_block_hash,
                    proof_count: record.proof_count,
                    proof_kinds: record.proof_kinds,
                    relay_submission_state: RelaySubmissionState::NotSubmitted,
                    relay_last_submitted_at_unix_ms: None,
                    relay_submission_attempts: 0,
                    relay_last_error: None,
                })
                .collect();
            return Ok(index);
        }

        // Phase 2B must not silently treat schema 1/2 packages as export-proof-bearing outputs.
        // The migration therefore preserves only coarse staged progress and drops all certified
        // claims so the exporter rebuilds schema 3/4 packages from chain state.
        let mut index = ExporterIndex::new(domain_id);
        index.latest_staged_epoch = latest_staged_epoch;
        Ok(index)
    }

    fn build_manifest(
        &self,
        package: &CertifiedSummaryPackage,
        target_domain: DomainId,
        export_ids: &[ExportId],
        scale_path: &Path,
    ) -> anyhow::Result<PackageManifest> {
        let SummaryCertificate::GrandpaV1(certificate) = &package.certificate;
        let summary_proof = decode_summary_header_storage_proof(
            package
                .inclusion_proofs
                .first()
                .ok_or_else(|| anyhow!("package is missing inclusion_proofs[0]"))?,
        )?;
        let proof_kinds = decode_proof_kinds(&package.inclusion_proofs[1..])?;

        Ok(PackageManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            domain_id: self.domain_id,
            target_domain,
            epoch_id: package.header.epoch_id,
            summary_hash: hex_hash(package.header.summary_hash),
            package_hash: hex_hash(package.package_hash),
            export_ids: export_ids.iter().map(|hash| hex_hash(*hash)).collect(),
            export_count: export_ids.len(),
            proof_count: package.inclusion_proofs.len(),
            proof_kinds,
            target_block_number: certificate.target_block_number,
            target_block_hash: hex_hash(certificate.target_block_hash),
            proof_block_number: certificate.proof_block_number,
            proof_block_hash: hex_hash(certificate.proof_block_hash),
            grandpa_set_id: certificate.grandpa_set_id,
            justification_len: certificate.justification.len(),
            ancestry_header_count: certificate.ancestry_headers.len(),
            summary_storage_key: hex_bytes(&summary_proof.storage_key),
            summary_storage_node_count: summary_proof.node_count(),
            summary_storage_total_bytes: summary_proof.total_proof_bytes(),
            summary_storage_header_len: summary_proof.proof_block_header.len(),
            artifacts_state: if package.artifacts.is_empty() {
                "empty".into()
            } else {
                "non_empty".into()
            },
            scale_path: scale_path.display().to_string(),
        })
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn package_scale_path(&self, epoch_id: EpochId, target_domain: DomainId) -> PathBuf {
        self.packages_dir.join(format!(
            "epoch-{epoch_id}-to-{}.scale",
            target_domain.as_str()
        ))
    }

    fn package_manifest_path(&self, epoch_id: EpochId, target_domain: DomainId) -> PathBuf {
        self.packages_dir.join(format!(
            "epoch-{epoch_id}-to-{}.json",
            target_domain.as_str()
        ))
    }

    fn archive_phase_2a_package_files_if_needed(&self, epoch_id: EpochId) -> anyhow::Result<()> {
        for extension in ["scale", "json"] {
            let legacy = self
                .packages_dir
                .join(format!("epoch-{epoch_id}.{extension}"));
            if legacy.exists() {
                let archived = self
                    .legacy_dir
                    .join(format!("phase2a-epoch-{epoch_id}.{extension}"));
                fs::rename(&legacy, &archived).with_context(|| {
                    format!(
                        "failed to archive legacy Phase 2A package {} to {}",
                        legacy.display(),
                        archived.display()
                    )
                })?;
            }
        }
        Ok(())
    }

    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        let tmp = path.with_extension(format!(
            "{}.tmp",
            path.extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("json")
        ));
        fs::write(&tmp, bytes)
            .with_context(|| format!("failed to write temp file {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| {
            format!(
                "failed to move temp file {} into place at {}",
                tmp.display(),
                path.display()
            )
        })
    }
}

pub fn decode_summary_header_storage_proof(
    bytes: &[u8],
) -> anyhow::Result<SummaryHeaderStorageProof> {
    match InclusionProof::decode(&mut &bytes[..])
        .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?
    {
        InclusionProof::SummaryHeaderStorageV1(proof) => Ok(proof),
        InclusionProof::ExportV1(_)
        | InclusionProof::FinalizedImportV1(_)
        | InclusionProof::GovernanceV1(_) => {
            bail!("expected summary-header storage proof at index 0")
        }
    }
}

pub fn decode_export_proofs(bytes: &[Vec<u8>]) -> anyhow::Result<Vec<ExportInclusionProof>> {
    bytes
        .iter()
        .map(|entry| {
            match InclusionProof::decode(&mut &entry[..])
                .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?
            {
                InclusionProof::ExportV1(proof) => Ok(proof),
                InclusionProof::SummaryHeaderStorageV1(_) => {
                    bail!("summary-header storage proof is only valid at inclusion_proofs[0]")
                }
                InclusionProof::FinalizedImportV1(_) => {
                    bail!("expected only export proofs after inclusion_proofs[0]")
                }
                InclusionProof::GovernanceV1(_) => {
                    bail!("expected only export proofs after inclusion_proofs[0]")
                }
            }
        })
        .collect()
}

pub fn decode_finalized_import_proofs(
    bytes: &[Vec<u8>],
) -> anyhow::Result<Vec<FinalizedImportInclusionProof>> {
    bytes
        .iter()
        .map(|entry| {
            match InclusionProof::decode(&mut &entry[..])
                .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?
            {
                InclusionProof::FinalizedImportV1(proof) => Ok(proof),
                InclusionProof::SummaryHeaderStorageV1(_) => {
                    bail!("summary-header storage proof is only valid at inclusion_proofs[0]")
                }
                InclusionProof::ExportV1(_) => {
                    bail!("expected only finalized-import proofs after inclusion_proofs[0]")
                }
                InclusionProof::GovernanceV1(_) => {
                    bail!("expected only finalized-import proofs after inclusion_proofs[0]")
                }
            }
        })
        .collect()
}

pub fn decode_governance_proofs(bytes: &[Vec<u8>]) -> anyhow::Result<Vec<GovernanceInclusionProof>> {
    bytes
        .iter()
        .map(|entry| {
            match InclusionProof::decode(&mut &entry[..])
                .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?
            {
                InclusionProof::GovernanceV1(proof) => Ok(proof),
                InclusionProof::SummaryHeaderStorageV1(_) => {
                    bail!("summary-header storage proof is only valid at inclusion_proofs[0]")
                }
                InclusionProof::ExportV1(_) => {
                    bail!("expected only governance proofs after inclusion_proofs[0]")
                }
                InclusionProof::FinalizedImportV1(_) => {
                    bail!("expected only governance proofs after inclusion_proofs[0]")
                }
            }
        })
        .collect()
}

fn decode_proof_kinds(bytes: &[Vec<u8>]) -> anyhow::Result<Vec<String>> {
    let mut kinds = vec!["summary_header_storage_v1".to_string()];
    for entry in bytes {
        let kind = match InclusionProof::decode(&mut &entry[..])
            .map_err(|error| anyhow!("failed to decode inclusion proof: {error}"))?
        {
            InclusionProof::SummaryHeaderStorageV1(_) => {
                bail!("summary-header storage proof is only valid at inclusion_proofs[0]")
            }
            InclusionProof::ExportV1(_) => "export_v1",
            InclusionProof::FinalizedImportV1(_) => "finalized_import_v1",
            InclusionProof::GovernanceV1(_) => "governance_v1",
        };
        kinds.push(kind.to_string());
    }
    Ok(kinds)
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
        build_governance_inclusion_proof, governance_proposal_id, DomainId, EpochSummaryHeader,
        GovernanceLeaf, GovernancePayload, GovernanceProposalLeaf, GovernanceProposalLeafHashInput,
        GrandpaFinalityCertificate, SummaryCertificate, SummaryCertificationBundle,
        SummaryHeaderStorageProof, EXPORT_INCLUSION_PROOF_VERSION,
        GOVERNANCE_PROPOSAL_LEAF_VERSION, SUMMARY_HEADER_STORAGE_PROOF_VERSION,
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

    fn sample_package() -> CertifiedSummaryPackage {
        let bundle = SummaryCertificationBundle {
            certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                version: 1,
                grandpa_set_id: 0,
                target_block_number: 13,
                target_block_hash: [11u8; 32],
                proof_block_number: 14,
                proof_block_hash: [12u8; 32],
                justification: vec![1, 2, 3],
                ancestry_headers: vec![vec![4, 5]],
            }),
            summary_header_storage_proof: SummaryHeaderStorageProof {
                version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                proof_block_number: 14,
                proof_block_hash: [12u8; 32],
                proof_block_header: vec![9, 9],
                storage_key: vec![1, 2, 3],
                trie_nodes: vec![vec![4, 5, 6]],
            },
        };
        CertifiedSummaryPackage::from_bundle_with_export_proofs(
            sample_header(),
            bundle,
            vec![ExportInclusionProof {
                version: EXPORT_INCLUSION_PROOF_VERSION,
                leaf: ialp_common_types::ExportLeaf {
                    version: 1,
                    export_id: [13u8; 32],
                    source_domain: DomainId::Earth,
                    target_domain: DomainId::Moon,
                    sender: [14u8; 32],
                    recipient: [15u8; 32],
                    amount: 22,
                    source_epoch_id: 3,
                    source_block_height: 11,
                    extrinsic_index: 0,
                    export_hash: [16u8; 32],
                },
                leaf_index: 0,
                leaf_count: 1,
                siblings: Vec::new(),
            }],
        )
    }

    fn sample_governance_package() -> CertifiedSummaryPackage {
        let bundle = SummaryCertificationBundle {
            certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                version: 1,
                grandpa_set_id: 0,
                target_block_number: 13,
                target_block_hash: [11u8; 32],
                proof_block_number: 14,
                proof_block_hash: [12u8; 32],
                justification: vec![1, 2, 3],
                ancestry_headers: vec![vec![4, 5]],
            }),
            summary_header_storage_proof: SummaryHeaderStorageProof {
                version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                proof_block_number: 14,
                proof_block_hash: [12u8; 32],
                proof_block_header: vec![9, 9],
                storage_key: vec![1, 2, 3],
                trie_nodes: vec![vec![4, 5, 6]],
            },
        };
        let leaf = GovernanceLeaf::ProposalV1(GovernanceProposalLeaf::from_hash_input(
            GovernanceProposalLeafHashInput {
                version: GOVERNANCE_PROPOSAL_LEAF_VERSION,
                proposal_id: governance_proposal_id(DomainId::Earth, 0),
                source_domain: DomainId::Earth,
                target_domain: DomainId::Moon,
                target_domains: vec![DomainId::Moon],
                proposer: [21u8; 32],
                payload_hash: GovernancePayload::SetProtocolVersion { new_version: 2 }
                    .payload_hash(),
                new_protocol_version: 2,
                created_epoch: 2,
                voting_start_epoch: 2,
                voting_end_epoch: 3,
                approval_epoch: 3,
                activation_epoch: 7,
            },
        ));
        CertifiedSummaryPackage::from_bundle_with_governance_proofs(
            sample_header(),
            bundle,
            vec![build_governance_inclusion_proof(core::slice::from_ref(&leaf), leaf.leaf_hash())
                .expect("proof")],
        )
    }

    #[test]
    fn decode_helpers_split_summary_and_export_proofs() {
        let package = sample_package();
        let summary = decode_summary_header_storage_proof(&package.inclusion_proofs[0])
            .expect("summary proof decodes");
        let exports =
            decode_export_proofs(&package.inclusion_proofs[1..]).expect("export proofs decode");

        assert_eq!(summary.version, SUMMARY_HEADER_STORAGE_PROOF_VERSION);
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].leaf.export_id, [13u8; 32]);
    }

    #[test]
    fn legacy_schema_indices_are_downgraded() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("index.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 2,
                "domain_id": "earth",
                "latest_staged_epoch": 4,
                "epochs": [],
            }))
            .expect("legacy json"),
        )
        .expect("write legacy index");

        let store = Store::new(root.path().to_path_buf(), DomainId::Earth).expect("store");
        let index = store.load_index().expect("index loads");

        assert_eq!(index.schema_version, INDEX_SCHEMA_VERSION);
        assert_eq!(index.latest_staged_epoch, Some(4));
        assert!(index.packages.is_empty());
    }

    #[test]
    fn package_paths_are_scoped_by_target_domain() {
        let store = Store::new(
            tempfile::tempdir().expect("tempdir").path().to_path_buf(),
            DomainId::Earth,
        )
        .expect("store");
        assert!(store
            .package_scale_path(7, DomainId::Moon)
            .display()
            .to_string()
            .contains("epoch-7-to-moon.scale"));
    }

    #[test]
    fn governance_decode_helper_extracts_governance_proofs() {
        let package = sample_governance_package();
        let proofs =
            decode_governance_proofs(&package.inclusion_proofs[1..]).expect("governance proofs");

        assert_eq!(proofs.len(), 1);
        assert!(matches!(proofs[0].leaf, GovernanceLeaf::ProposalV1(_)));
    }
}
