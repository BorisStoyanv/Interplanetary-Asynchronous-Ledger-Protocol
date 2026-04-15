use anyhow::{anyhow, bail, Context};
use codec::{Decode, Encode};
use ialp_common_types::{
    epoch_export_ids_storage_key, epoch_finalized_import_ids_storage_key,
    export_record_storage_key, observed_import_storage_key, summary_header_storage_key, EpochId,
    EpochSummaryHeader, ExportId, ExportRecord, ObservedImportRecord,
    SummaryCertificationReadiness,
};
use jsonrpsee::{
    core::{
        client::{ClientT, Subscription, SubscriptionClientT},
        rpc_params,
    },
    ws_client::{WsClient, WsClientBuilder},
};

pub struct NodeRpcClient {
    client: WsClient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Decode, Default)]
enum SummarySlotStatus {
    #[default]
    Reserved,
    Staged,
}

#[derive(Clone, Debug, PartialEq, Eq, Decode, Default)]
struct EpochAccumulators {
    state_root: [u8; 32],
    block_root: [u8; 32],
    tx_root: [u8; 32],
    event_root: [u8; 32],
    blocks_observed: u32,
    last_observed_block: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Decode, Default)]
struct SummarySlotRecord {
    epoch_id: EpochId,
    start_height: u32,
    end_height: Option<u32>,
    reserved_at_block: u32,
    staged_at_block_number: Option<u32>,
    last_touched_block: u32,
    header: Option<EpochSummaryHeader>,
    status: SummarySlotStatus,
    accumulators: EpochAccumulators,
}

#[derive(Clone, Debug)]
pub struct StagedSummaryView {
    pub header: EpochSummaryHeader,
    pub staged_at_block_number: u32,
}

impl StagedSummaryView {
    pub fn json_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "epoch_id": self.header.epoch_id,
            "summary_hash": format!("0x{}", hex::encode(self.header.summary_hash)),
            "staged_at_block_number": self.staged_at_block_number,
            "start_block_height": self.header.start_block_height,
            "end_block_height": self.header.end_block_height,
            "export_root": format!("0x{}", hex::encode(self.header.export_root)),
        })
    }
}

impl NodeRpcClient {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let client = WsClientBuilder::default()
            .build(url)
            .await
            .with_context(|| format!("failed to connect exporter websocket to {url}"))?;
        Ok(Self { client })
    }

    pub async fn latest_staged_summary(&self) -> anyhow::Result<Option<StagedSummaryView>> {
        let Some(header) = self
            .load_storage_value::<EpochSummaryHeader>(latest_summary_header_storage_key())
            .await?
        else {
            return Ok(None);
        };

        self.summary_by_epoch(header.epoch_id).await
    }

    pub async fn summary_by_epoch(
        &self,
        epoch_id: EpochId,
    ) -> anyhow::Result<Option<StagedSummaryView>> {
        let header = self
            .load_storage_value::<EpochSummaryHeader>(summary_header_storage_key(epoch_id))
            .await?;
        let slot = self
            .load_storage_value::<SummarySlotRecord>(summary_slot_storage_key(epoch_id))
            .await?;

        match (header, slot) {
            (Some(header), Some(slot)) => {
                let staged = slot.staged_at_block_number.ok_or_else(|| {
                    anyhow!("epoch {epoch_id} slot exists but is missing staged_at_block_number")
                })?;
                Ok(Some(StagedSummaryView {
                    header,
                    staged_at_block_number: staged,
                }))
            }
            (None, None) => Ok(None),
            _ => bail!("epoch {epoch_id} header/slot storage is inconsistent"),
        }
    }

    pub async fn certification_readiness(
        &self,
        epoch_id: EpochId,
    ) -> anyhow::Result<SummaryCertificationReadiness> {
        self.client
            .request("ialp_summary_certificationReadiness", rpc_params![epoch_id])
            .await
            .with_context(|| {
                format!("failed to query certification readiness for epoch {epoch_id}")
            })
    }

    pub async fn subscribe_finalized_heads(
        &self,
    ) -> anyhow::Result<Subscription<serde_json::Value>> {
        self.client
            .subscribe(
                "chain_subscribeFinalizedHeads",
                rpc_params![],
                "chain_unsubscribeFinalizedHeads",
            )
            .await
            .context("failed to subscribe to finalized heads")
    }

    pub async fn epoch_export_ids(&self, epoch_id: EpochId) -> anyhow::Result<Vec<ExportId>> {
        Ok(self
            .load_storage_value::<Vec<ExportId>>(epoch_export_ids_storage_key(epoch_id))
            .await?
            .unwrap_or_default())
    }

    pub async fn export_record(&self, export_id: ExportId) -> anyhow::Result<Option<ExportRecord>> {
        self.load_storage_value::<ExportRecord>(export_record_storage_key(export_id))
            .await
    }

    pub async fn epoch_exports(&self, epoch_id: EpochId) -> anyhow::Result<Vec<ExportRecord>> {
        let export_ids = self.epoch_export_ids(epoch_id).await?;
        let mut exports = Vec::with_capacity(export_ids.len());
        for export_id in export_ids {
            let record = self
                .export_record(export_id)
                .await?
                .ok_or_else(|| anyhow!("missing export record 0x{}", hex::encode(export_id)))?;
            exports.push(record);
        }
        Ok(exports)
    }

    pub async fn epoch_finalized_import_ids(
        &self,
        epoch_id: EpochId,
    ) -> anyhow::Result<Vec<ExportId>> {
        Ok(self
            .load_storage_value::<Vec<ExportId>>(epoch_finalized_import_ids_storage_key(epoch_id))
            .await?
            .unwrap_or_default())
    }

    pub async fn observed_import(
        &self,
        export_id: ExportId,
    ) -> anyhow::Result<Option<ObservedImportRecord>> {
        self.load_storage_value::<ObservedImportRecord>(observed_import_storage_key(export_id))
            .await
    }

    pub async fn epoch_finalized_imports(
        &self,
        epoch_id: EpochId,
    ) -> anyhow::Result<Vec<ObservedImportRecord>> {
        let export_ids = self.epoch_finalized_import_ids(epoch_id).await?;
        let mut records = Vec::with_capacity(export_ids.len());
        for export_id in export_ids {
            let record = self
                .observed_import(export_id)
                .await?
                .ok_or_else(|| anyhow!("missing observed import record 0x{}", hex::encode(export_id)))?;
            records.push(record);
        }
        Ok(records)
    }

    async fn load_storage_value<T: Decode>(&self, key: Vec<u8>) -> anyhow::Result<Option<T>> {
        let hex_key = format!("0x{}", hex::encode(key));
        let response: Option<String> = self
            .client
            .request("state_getStorage", rpc_params![hex_key])
            .await
            .context("failed to query storage")?;

        response
            .map(|value| {
                let raw = decode_hex_bytes(&value)?;
                T::decode(&mut &raw[..]).map_err(|error| anyhow!("scale decode failed: {error}"))
            })
            .transpose()
    }
}

fn latest_summary_header_storage_key() -> Vec<u8> {
    let mut key = Vec::with_capacity(32);
    key.extend_from_slice(&sp_io::hashing::twox_128(b"Epochs"));
    key.extend_from_slice(&sp_io::hashing::twox_128(b"LatestSummaryHeader"));
    key
}

fn summary_slot_storage_key(epoch_id: EpochId) -> Vec<u8> {
    let mut key = Vec::with_capacity(64);
    key.extend_from_slice(&sp_io::hashing::twox_128(b"Epochs"));
    key.extend_from_slice(&sp_io::hashing::twox_128(b"SummarySlots"));
    let encoded = epoch_id.encode();
    key.extend_from_slice(&sp_io::hashing::blake2_128(&encoded));
    key.extend_from_slice(&encoded);
    key
}

fn decode_hex_bytes(value: &str) -> anyhow::Result<Vec<u8>> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(trimmed).with_context(|| format!("failed to decode hex bytes '{value}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ialp_common_types::{observed_import_storage_key, DomainId};

    #[test]
    fn transfer_storage_keys_are_distinct() {
        let export_id = [7u8; 32];
        assert_ne!(
            epoch_export_ids_storage_key(3),
            export_record_storage_key(export_id)
        );
        assert_ne!(
            export_record_storage_key(export_id),
            observed_import_storage_key(export_id)
        );
    }

    #[test]
    fn staged_summary_json_includes_export_root() {
        let summary = StagedSummaryView {
            header: EpochSummaryHeader {
                version: 1,
                domain_id: DomainId::Earth,
                epoch_id: 4,
                prev_summary_hash: [0u8; 32],
                start_block_height: 1,
                end_block_height: 3,
                state_root: [1u8; 32],
                block_root: [2u8; 32],
                tx_root: [3u8; 32],
                event_root: [4u8; 32],
                export_root: [5u8; 32],
                import_root: [6u8; 32],
                governance_root: [7u8; 32],
                validator_set_hash: [8u8; 32],
                summary_hash: [9u8; 32],
            },
            staged_at_block_number: 4,
        };

        let json = summary.json_summary();
        assert_eq!(json["epoch_id"], 4);
        assert_eq!(
            json["export_root"],
            serde_json::Value::String(format!("0x{}", hex::encode([5u8; 32])))
        );
    }
}
