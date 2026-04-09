use anyhow::{anyhow, bail, Context};
use blake2::{
    digest::{Update, VariableOutput},
    Blake2bVar,
};
use codec::{Decode, Encode};
use ialp_common_types::{
    summary_header_storage_key, EpochId, EpochSummaryHeader, SummaryCertificationReadiness,
};
use jsonrpsee::{
    core::{
        client::{ClientT, Subscription, SubscriptionClientT},
        rpc_params,
    },
    ws_client::{WsClient, WsClientBuilder},
};
use std::hash::Hasher;
use twox_hash::XxHash64;

pub struct NodeRpcClient {
    client: WsClient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, Default)]
enum SummarySlotStatus {
    #[default]
    Reserved,
    Staged,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, Default)]
struct EpochAccumulators {
    state_root: [u8; 32],
    block_root: [u8; 32],
    tx_root: [u8; 32],
    event_root: [u8; 32],
    blocks_observed: u32,
    last_observed_block: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, Default)]
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
    storage_prefix(b"Epochs", b"LatestSummaryHeader")
}

fn summary_slot_storage_key(epoch_id: EpochId) -> Vec<u8> {
    let mut key = storage_prefix(b"Epochs", b"SummarySlots");
    key.extend(blake2_128_concat(&epoch_id.encode()));
    key
}

fn storage_prefix(pallet: &[u8], storage: &[u8]) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(32);
    prefix.extend_from_slice(&twox_128(pallet));
    prefix.extend_from_slice(&twox_128(storage));
    prefix
}

fn blake2_128_concat(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(16 + data.len());
    output.extend_from_slice(&blake2_128(data));
    output.extend_from_slice(data);
    output
}

fn blake2_128(data: &[u8]) -> [u8; 16] {
    let mut output = [0u8; 16];
    let mut hasher = Blake2bVar::new(output.len()).expect("blake2 output length is valid");
    hasher.update(data);
    hasher
        .finalize_variable(&mut output)
        .expect("output buffer length matches configured digest length");
    output
}

fn twox_128(data: &[u8]) -> [u8; 16] {
    let mut output = [0u8; 16];
    output[..8].copy_from_slice(&twox_64_with_seed(data, 0).to_le_bytes());
    output[8..].copy_from_slice(&twox_64_with_seed(data, 1).to_le_bytes());
    output
}

fn twox_64_with_seed(data: &[u8], seed: u64) -> u64 {
    let mut hasher = XxHash64::with_seed(seed);
    hasher.write(data);
    hasher.finish()
}

fn decode_hex_bytes(value: &str) -> anyhow::Result<Vec<u8>> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(trimmed).with_context(|| format!("failed to decode hex bytes '{value}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_keys_are_distinct_for_slot_and_header() {
        assert_ne!(summary_header_storage_key(1), summary_slot_storage_key(1));
    }

    #[test]
    fn decodes_hex_storage_response() {
        let bytes = decode_hex_bytes("0x010203").expect("hex should decode");
        assert_eq!(bytes, vec![1, 2, 3]);
    }
}
