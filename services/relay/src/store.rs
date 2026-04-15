use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use codec::Encode;
use ialp_common_types::{DomainId, ImporterPackageState, RelayPackageEnvelopeV1};
use serde::{Deserialize, Serialize};

const INDEX_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelayQueueState {
    Queued,
    Scheduled,
    BlockedByBlackout,
    InDelivery,
    Delivered,
    ImporterAcked,
    Retrying,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayIndex {
    pub schema_version: u16,
    pub packages: Vec<RelayPackageRecord>,
}

impl RelayIndex {
    pub fn new() -> Self {
        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            packages: Vec::new(),
        }
    }

    pub fn record(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        package_hash: [u8; 32],
    ) -> Option<&RelayPackageRecord> {
        let package_hash = hex_hash(package_hash);
        self.packages.iter().find(|record| {
            record.source_domain == source_domain
                && record.target_domain == target_domain
                && record.epoch_id == epoch_id
                && record.package_hash == package_hash
        })
    }

    pub fn record_mut(
        &mut self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        package_hash: [u8; 32],
    ) -> Option<&mut RelayPackageRecord> {
        let package_hash = hex_hash(package_hash);
        self.packages.iter_mut().find(|record| {
            record.source_domain == source_domain
                && record.target_domain == target_domain
                && record.epoch_id == epoch_id
                && record.package_hash == package_hash
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayPackageRecord {
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub epoch_id: u64,
    pub summary_hash: String,
    pub package_hash: String,
    pub export_count: u32,
    pub state: RelayQueueState,
    pub relay_submitted_at_unix_ms: u64,
    pub relay_accepted_at_unix_ms: u64,
    #[serde(default)]
    pub ever_blocked_by_blackout: bool,
    pub next_delivery_at_unix_ms: Option<u64>,
    pub next_ack_poll_at_unix_ms: Option<u64>,
    pub delivery_attempts: u32,
    pub last_delivery_error: Option<String>,
    pub delivered_at_unix_ms: Option<u64>,
    pub completed_at_unix_ms: Option<u64>,
    pub importer_state: Option<ImporterPackageState>,
    pub importer_reason: Option<String>,
    pub last_importer_status_at_unix_ms: Option<u64>,
    pub payload_path: String,
    pub entry_path: String,
}

impl RelayPackageRecord {
    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "source_domain": self.source_domain,
            "target_domain": self.target_domain,
            "epoch_id": self.epoch_id,
            "summary_hash": self.summary_hash,
            "package_hash": self.package_hash,
            "export_count": self.export_count,
            "state": self.state,
            "relay_submitted_at_unix_ms": self.relay_submitted_at_unix_ms,
            "relay_accepted_at_unix_ms": self.relay_accepted_at_unix_ms,
            "ever_blocked_by_blackout": self.ever_blocked_by_blackout,
            "next_delivery_at_unix_ms": self.next_delivery_at_unix_ms,
            "next_ack_poll_at_unix_ms": self.next_ack_poll_at_unix_ms,
            "delivery_attempts": self.delivery_attempts,
            "last_delivery_error": self.last_delivery_error,
            "delivered_at_unix_ms": self.delivered_at_unix_ms,
            "completed_at_unix_ms": self.completed_at_unix_ms,
            "importer_state": self.importer_state,
            "importer_reason": self.importer_reason,
            "last_importer_status_at_unix_ms": self.last_importer_status_at_unix_ms,
            "payload_path": self.payload_path,
            "entry_path": self.entry_path,
        })
    }
}

pub struct Store {
    root: PathBuf,
    entries_dir: PathBuf,
    payloads_dir: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> anyhow::Result<Self> {
        let entries_dir = root.join("entries");
        let payloads_dir = root.join("payloads");
        fs::create_dir_all(&entries_dir).with_context(|| {
            format!(
                "failed to create relay entries dir {}",
                entries_dir.display()
            )
        })?;
        fs::create_dir_all(&payloads_dir).with_context(|| {
            format!(
                "failed to create relay payload dir {}",
                payloads_dir.display()
            )
        })?;
        Ok(Self {
            root,
            entries_dir,
            payloads_dir,
        })
    }

    pub fn load_index(&self) -> anyhow::Result<RelayIndex> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(RelayIndex::new());
        }
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read relay index {}", path.display()))?;
        let index: RelayIndex = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to decode relay index {}", path.display()))?;
        if index.schema_version != INDEX_SCHEMA_VERSION {
            bail!(
                "unsupported relay index schema version {}; expected {}",
                index.schema_version,
                INDEX_SCHEMA_VERSION
            );
        }
        Ok(index)
    }

    pub fn save_index(&self, index: &RelayIndex) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(index).context("failed to serialize relay index")?;
        self.atomic_write(&self.index_path(), &bytes)
    }

    pub fn accept_envelope(
        &self,
        index: &mut RelayIndex,
        envelope: &RelayPackageEnvelopeV1,
        relay_accepted_at_unix_ms: u64,
    ) -> anyhow::Result<(RelayPackageRecord, bool)> {
        let encoded_envelope = envelope.encode();
        if let Some(existing) = index.record(
            envelope.source_domain,
            envelope.target_domain,
            envelope.epoch_id,
            envelope.package_hash,
        ) {
            let current = fs::read(&existing.payload_path)
                .with_context(|| format!("failed to read {}", existing.payload_path))?;
            if current != encoded_envelope {
                bail!(
                    "relay package identity already exists with different payload bytes for {}",
                    existing.package_hash
                );
            }
            return Ok((existing.clone(), true));
        }

        let payload_path = self.payload_path(
            envelope.source_domain,
            envelope.target_domain,
            envelope.epoch_id,
            envelope.package_hash,
        );
        let entry_path = self.entry_path(
            envelope.source_domain,
            envelope.target_domain,
            envelope.epoch_id,
            envelope.package_hash,
        );
        self.atomic_write(&payload_path, &encoded_envelope)?;

        let record = RelayPackageRecord {
            source_domain: envelope.source_domain,
            target_domain: envelope.target_domain,
            epoch_id: envelope.epoch_id,
            summary_hash: hex_hash(envelope.summary_hash),
            package_hash: hex_hash(envelope.package_hash),
            export_count: envelope.export_count,
            state: RelayQueueState::Queued,
            relay_submitted_at_unix_ms: envelope.relay_submitted_at_unix_ms,
            relay_accepted_at_unix_ms,
            ever_blocked_by_blackout: false,
            next_delivery_at_unix_ms: None,
            next_ack_poll_at_unix_ms: None,
            delivery_attempts: 0,
            last_delivery_error: None,
            delivered_at_unix_ms: None,
            completed_at_unix_ms: None,
            importer_state: None,
            importer_reason: None,
            last_importer_status_at_unix_ms: None,
            payload_path: payload_path.display().to_string(),
            entry_path: entry_path.display().to_string(),
        };
        self.persist_record(index, record.clone())?;
        Ok((record, false))
    }

    pub fn persist_record(
        &self,
        index: &mut RelayIndex,
        record: RelayPackageRecord,
    ) -> anyhow::Result<()> {
        let path = PathBuf::from(&record.entry_path);
        let bytes =
            serde_json::to_vec_pretty(&record).context("failed to serialize relay entry")?;
        self.atomic_write(&path, &bytes)?;

        match index.record_mut(
            record.source_domain,
            record.target_domain,
            record.epoch_id,
            decode_hex_hash(&record.package_hash)?,
        ) {
            Some(current) => *current = record,
            None => index.packages.push(record),
        }
        Ok(())
    }

    pub fn load_payload(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        package_hash: [u8; 32],
    ) -> anyhow::Result<Vec<u8>> {
        let path = self.payload_path(source_domain, target_domain, epoch_id, package_hash);
        fs::read(&path).with_context(|| format!("failed to read relay payload {}", path.display()))
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn entry_path(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        package_hash: [u8; 32],
    ) -> PathBuf {
        self.entries_dir.join(format!(
            "{}-{}-epoch-{}-{}.json",
            source_domain.as_str(),
            target_domain.as_str(),
            epoch_id,
            hex::encode(package_hash)
        ))
    }

    fn payload_path(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: u64,
        package_hash: [u8; 32],
    ) -> PathBuf {
        self.payloads_dir.join(format!(
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
        .map_err(|_| anyhow::anyhow!("expected 32-byte hash string '{}'", value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ialp_common_types::RelayPackageEnvelopeV1;

    fn sample_envelope() -> RelayPackageEnvelopeV1 {
        RelayPackageEnvelopeV1::new(
            DomainId::Earth,
            DomainId::Moon,
            4,
            [1u8; 32],
            [2u8; 32],
            vec![1, 2, 3],
            2,
            123,
        )
    }

    #[test]
    fn relay_accept_is_idempotent_for_identical_payloads() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = Store::new(root.path().to_path_buf()).expect("store");
        let mut index = store.load_index().expect("index");
        let envelope = sample_envelope();

        let (_, idempotent_first) = store
            .accept_envelope(&mut index, &envelope, 999)
            .expect("first accept");
        let (_, idempotent_second) = store
            .accept_envelope(&mut index, &envelope, 1_000)
            .expect("second accept");

        assert!(!idempotent_first);
        assert!(idempotent_second);
        assert_eq!(index.packages.len(), 1);
    }
}
