use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context};
use ialp_common_types::{DomainId, ExportId, ImporterPackageState};
use serde::{Deserialize, Serialize};

const INDEX_SCHEMA_VERSION: u16 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImporterIndex {
    pub schema_version: u16,
    pub domain_id: DomainId,
    pub imports: Vec<ImportRecord>,
    pub packages: Vec<PackageRecord>,
}

impl ImporterIndex {
    pub fn new(domain_id: DomainId) -> Self {
        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            domain_id,
            imports: Vec::new(),
            packages: Vec::new(),
        }
    }

    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "schema_version": self.schema_version,
            "domain_id": self.domain_id,
            "import_records": self.imports.len(),
            "package_records": self.packages.len(),
        })
    }

    pub fn record(&self, export_id: ExportId) -> Option<&ImportRecord> {
        let export_id = hex_hash(export_id);
        self.imports
            .iter()
            .find(|record| record.export_id == export_id)
    }

    pub fn package(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        package_hash: [u8; 32],
    ) -> Option<&PackageRecord> {
        let package_hash = hex_hash(package_hash);
        self.packages.iter().find(|record| {
            record.source_domain == source_domain
                && record.target_domain == target_domain
                && record.epoch_id == epoch_id
                && record.package_hash == package_hash
        })
    }

    pub fn upsert(&mut self, record: ImportRecord) {
        match self
            .imports
            .iter_mut()
            .find(|current| current.export_id == record.export_id)
        {
            Some(current) => *current = record,
            None => self.imports.push(record),
        }
    }

    pub fn upsert_package(&mut self, record: PackageRecord) {
        match self.packages.iter_mut().find(|current| {
            current.source_domain == record.source_domain
                && current.target_domain == record.target_domain
                && current.epoch_id == record.epoch_id
                && current.package_hash == record.package_hash
        }) {
            Some(current) => *current = record,
            None => self.packages.push(record),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Verified,
    Invalid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateStatus {
    NotDuplicate,
    DuplicateLocal,
    DuplicateRemote,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionStatus {
    RemoteObserved,
    RemoteFinalized,
    SourceResolved,
    SkippedDuplicate,
    Rejected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportRecord {
    pub export_id: String,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub summary_hash: String,
    pub package_hash: String,
    pub verification_status: VerificationStatus,
    pub duplicate_status: DuplicateStatus,
    pub submission_status: SubmissionStatus,
    pub tx_hash: Option<String>,
    pub reason: Option<String>,
}

impl ImportRecord {
    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "export_id": self.export_id,
            "source_domain": self.source_domain,
            "target_domain": self.target_domain,
            "summary_hash": self.summary_hash,
            "package_hash": self.package_hash,
            "verification_status": self.verification_status,
            "duplicate_status": self.duplicate_status,
            "submission_status": self.submission_status,
            "tx_hash": self.tx_hash,
            "reason": self.reason,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageRecord {
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub epoch_id: u64,
    pub summary_hash: String,
    pub package_hash: String,
    pub state: ImporterPackageState,
    pub reason: Option<String>,
    pub export_count: u32,
    pub received_at_unix_ms: u64,
    pub last_updated_at_unix_ms: u64,
    pub completed_at_unix_ms: Option<u64>,
    pub tx_hashes: Vec<String>,
    pub payload_path: String,
}

impl PackageRecord {
    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "source_domain": self.source_domain,
            "target_domain": self.target_domain,
            "epoch_id": self.epoch_id,
            "summary_hash": self.summary_hash,
            "package_hash": self.package_hash,
            "state": self.state,
            "reason": self.reason,
            "export_count": self.export_count,
            "received_at_unix_ms": self.received_at_unix_ms,
            "last_updated_at_unix_ms": self.last_updated_at_unix_ms,
            "completed_at_unix_ms": self.completed_at_unix_ms,
            "tx_hashes": self.tx_hashes,
            "payload_path": self.payload_path,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ImporterIndexV1 {
    pub schema_version: u16,
    pub domain_id: DomainId,
    pub imports: Vec<ImportRecord>,
}

pub struct Store {
    root: PathBuf,
    imports_dir: PathBuf,
    packages_dir: PathBuf,
    domain_id: DomainId,
}

impl Store {
    pub fn new(root: PathBuf, domain_id: DomainId) -> anyhow::Result<Self> {
        let imports_dir = root.join("imports");
        let packages_dir = root.join("packages");
        fs::create_dir_all(&imports_dir).with_context(|| {
            format!(
                "failed to create importer import storage dir {}",
                imports_dir.display()
            )
        })?;
        fs::create_dir_all(&packages_dir).with_context(|| {
            format!(
                "failed to create importer package storage dir {}",
                packages_dir.display()
            )
        })?;
        Ok(Self {
            root,
            imports_dir,
            packages_dir,
            domain_id,
        })
    }

    pub fn load_index(&self) -> anyhow::Result<ImporterIndex> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(ImporterIndex::new(self.domain_id));
        }
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read importer index {}", path.display()))?;
        let raw: serde_json::Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to decode importer index {}", path.display()))?;
        let schema_version = raw
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u16;
        let mut index = match schema_version {
            INDEX_SCHEMA_VERSION => serde_json::from_value(raw)
                .with_context(|| format!("failed to decode importer index {}", path.display()))?,
            1 => {
                let legacy: ImporterIndexV1 = serde_json::from_value(raw)
                    .context("failed to decode legacy importer index")?;
                let mut migrated = ImporterIndex::new(legacy.domain_id);
                migrated.imports = legacy.imports;
                migrated
            }
            other => bail!(
                "unsupported importer index schema version {}; expected 1 or {}",
                other,
                INDEX_SCHEMA_VERSION
            ),
        };
        if index.domain_id != self.domain_id {
            bail!(
                "importer index domain {} does not match requested domain {}",
                index.domain_id,
                self.domain_id
            );
        }
        for package in &mut index.packages {
            if package.state == ImporterPackageState::Verifying {
                package.state = ImporterPackageState::Received;
            }
        }
        Ok(index)
    }

    pub fn save_index(&self, index: &ImporterIndex) -> anyhow::Result<()> {
        if index.domain_id != self.domain_id {
            bail!(
                "importer index domain {} does not match store domain {}",
                index.domain_id,
                self.domain_id
            );
        }
        let bytes = serde_json::to_vec_pretty(index)?;
        self.atomic_write(&self.index_path(), &bytes)
    }

    pub fn persist_record(
        &self,
        index: &mut ImporterIndex,
        record: ImportRecord,
    ) -> anyhow::Result<()> {
        let path = self.record_path(&record.export_id);
        let bytes = serde_json::to_vec_pretty(&record)?;
        self.atomic_write(&path, &bytes)?;
        index.upsert(record);
        Ok(())
    }

    pub fn persist_package(
        &self,
        index: &mut ImporterIndex,
        record: PackageRecord,
        payload_bytes: Option<&[u8]>,
    ) -> anyhow::Result<()> {
        if let Some(bytes) = payload_bytes {
            let payload_path = PathBuf::from(&record.payload_path);
            if payload_path.exists() {
                let current = fs::read(&payload_path)
                    .with_context(|| format!("failed to read {}", payload_path.display()))?;
                if current != bytes {
                    bail!("importer package identity already exists with different payload bytes");
                }
            } else {
                self.atomic_write(&payload_path, bytes)?;
            }
        }

        let path = self.package_record_path(
            record.source_domain,
            record.target_domain,
            record.epoch_id,
            &record.package_hash,
        );
        let bytes = serde_json::to_vec_pretty(&record)?;
        self.atomic_write(&path, &bytes)?;
        index.upsert_package(record);
        Ok(())
    }

    pub fn load_record(&self, export_id: ExportId) -> anyhow::Result<Option<ImportRecord>> {
        let path = self.record_path(&hex_hash(export_id));
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read importer record {}", path.display()))?;
        let record: ImportRecord = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to decode importer record {}", path.display()))?;
        Ok(Some(record))
    }

    pub fn load_package(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        package_hash: [u8; 32],
    ) -> anyhow::Result<Option<PackageRecord>> {
        let path = self.package_record_path(
            source_domain,
            target_domain,
            epoch_id,
            &hex_hash(package_hash),
        );
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read importer package {}", path.display()))?;
        let record: PackageRecord = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to decode importer package {}", path.display()))?;
        Ok(Some(record))
    }

    pub fn load_package_bytes(&self, package_record: &PackageRecord) -> anyhow::Result<Vec<u8>> {
        fs::read(&package_record.payload_path).with_context(|| {
            format!(
                "failed to read package payload {}",
                package_record.payload_path
            )
        })
    }

    pub fn build_package_record(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        summary_hash: [u8; 32],
        package_hash: [u8; 32],
        export_count: u32,
        received_at_unix_ms: u64,
    ) -> PackageRecord {
        PackageRecord {
            source_domain,
            target_domain,
            epoch_id,
            summary_hash: hex_hash(summary_hash),
            package_hash: hex_hash(package_hash),
            state: ImporterPackageState::Received,
            reason: None,
            export_count,
            received_at_unix_ms,
            last_updated_at_unix_ms: received_at_unix_ms,
            completed_at_unix_ms: None,
            tx_hashes: Vec::new(),
            payload_path: self
                .package_payload_path(source_domain, target_domain, epoch_id, package_hash)
                .display()
                .to_string(),
        }
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn record_path(&self, export_id: &str) -> PathBuf {
        self.imports_dir.join(format!("{export_id}.json"))
    }

    fn package_record_path(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        package_hash_hex: &str,
    ) -> PathBuf {
        let trimmed = package_hash_hex
            .strip_prefix("0x")
            .unwrap_or(package_hash_hex);
        self.packages_dir.join(format!(
            "{}-{}-epoch-{}-{}.json",
            source_domain.as_str(),
            target_domain.as_str(),
            epoch_id,
            trimmed
        ))
    }

    fn package_payload_path(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        package_hash: [u8; 32],
    ) -> PathBuf {
        self.packages_dir.join(format!(
            "{}-{}-epoch-{}-{}.scale",
            source_domain.as_str(),
            target_domain.as_str(),
            epoch_id,
            hex::encode(package_hash)
        ))
    }

    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        let tmp = path.with_extension("tmp");
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

pub fn hex_hash(hash: [u8; 32]) -> String {
    format!("0x{}", hex::encode(hash))
}

pub fn decode_hex_hash(value: &str) -> anyhow::Result<[u8; 32]> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    let bytes =
        hex::decode(trimmed).with_context(|| format!("failed to decode hex hash '{}'", value))?;
    bytes
        .try_into()
        .map_err(|_| anyhow!("expected a 32-byte hash string"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> ImportRecord {
        ImportRecord {
            export_id: "0x1111111111111111111111111111111111111111111111111111111111111111".into(),
            source_domain: DomainId::Earth,
            target_domain: DomainId::Moon,
            summary_hash: "0x2222222222222222222222222222222222222222222222222222222222222222"
                .into(),
            package_hash: "0x3333333333333333333333333333333333333333333333333333333333333333"
                .into(),
            verification_status: VerificationStatus::Verified,
            duplicate_status: DuplicateStatus::NotDuplicate,
            submission_status: SubmissionStatus::RemoteObserved,
            tx_hash: Some("0xabc".into()),
            reason: Some("stored".into()),
        }
    }

    #[test]
    fn persist_and_reload_record_by_export_id() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = Store::new(root.path().to_path_buf(), DomainId::Moon).expect("store");
        let mut index = store.load_index().expect("index");
        let record = sample_record();

        store
            .persist_record(&mut index, record.clone())
            .expect("record persisted");
        store.save_index(&index).expect("index saved");

        let loaded = store
            .load_record([0x11u8; 32])
            .expect("load record")
            .expect("record exists");
        assert_eq!(loaded.export_id, record.export_id);
        assert_eq!(
            index
                .record([0x11u8; 32])
                .expect("index record")
                .package_hash,
            record.package_hash
        );
    }

    #[test]
    fn package_records_persist_payloads_and_survive_reload() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = Store::new(root.path().to_path_buf(), DomainId::Moon).expect("store");
        let mut index = store.load_index().expect("index");
        let record = store.build_package_record(
            DomainId::Earth,
            DomainId::Moon,
            7,
            [1u8; 32],
            [2u8; 32],
            3,
            100,
        );

        store
            .persist_package(&mut index, record.clone(), Some(&[1, 2, 3]))
            .expect("package persisted");
        store.save_index(&index).expect("index saved");

        let loaded = store
            .load_package(DomainId::Earth, DomainId::Moon, 7, [2u8; 32])
            .expect("load package")
            .expect("package exists");
        assert_eq!(loaded.package_hash, record.package_hash);
        assert_eq!(
            store.load_package_bytes(&loaded).expect("payload"),
            vec![1, 2, 3]
        );
    }
}
