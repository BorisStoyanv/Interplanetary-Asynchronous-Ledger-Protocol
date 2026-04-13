use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use ialp_common_types::{DomainId, ExportId};
use serde::{Deserialize, Serialize};

const INDEX_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImporterIndex {
    pub schema_version: u16,
    pub domain_id: DomainId,
    pub imports: Vec<ImportRecord>,
}

impl ImporterIndex {
    pub fn new(domain_id: DomainId) -> Self {
        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            domain_id,
            imports: Vec::new(),
        }
    }

    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "schema_version": self.schema_version,
            "domain_id": self.domain_id,
            "records": self.imports.len(),
        })
    }

    pub fn record(&self, export_id: ExportId) -> Option<&ImportRecord> {
        let export_id = hex_hash(export_id);
        self.imports
            .iter()
            .find(|record| record.export_id == export_id)
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

pub struct Store {
    root: PathBuf,
    imports_dir: PathBuf,
    domain_id: DomainId,
}

impl Store {
    pub fn new(root: PathBuf, domain_id: DomainId) -> anyhow::Result<Self> {
        let imports_dir = root.join("imports");
        fs::create_dir_all(&imports_dir).with_context(|| {
            format!(
                "failed to create importer storage dir {}",
                imports_dir.display()
            )
        })?;
        Ok(Self {
            root,
            imports_dir,
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
        let index: ImporterIndex = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to decode importer index {}", path.display()))?;
        if index.domain_id != self.domain_id {
            bail!(
                "importer index domain {} does not match requested domain {}",
                index.domain_id,
                self.domain_id
            );
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

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn record_path(&self, export_id: &str) -> PathBuf {
        self.imports_dir.join(format!("{export_id}.json"))
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

fn hex_hash(hash: [u8; 32]) -> String {
    format!("0x{}", hex::encode(hash))
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
}
