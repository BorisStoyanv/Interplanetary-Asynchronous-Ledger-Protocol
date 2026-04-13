use anyhow::{anyhow, Context};
use codec::Decode;
use ialp_common_types::{
    importer_account_storage_key, observed_import_storage_key, ExportId, ObservedImportRecord,
};
use jsonrpsee::{
    core::{client::ClientT, rpc_params},
    ws_client::{WsClient, WsClientBuilder},
};
use serde::Deserialize;
use sp_core::{crypto::Ss58Codec, sr25519::Pair as Sr25519Pair, Pair, H256};

pub struct NodeRpcClient {
    client: WsClient,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeVersionView {
    pub spec_version: u32,
    pub transaction_version: u32,
}

impl NodeRpcClient {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let client = WsClientBuilder::default()
            .build(url)
            .await
            .with_context(|| format!("failed to connect importer websocket to {url}"))?;
        Ok(Self { client })
    }

    pub async fn runtime_version(&self) -> anyhow::Result<RuntimeVersionView> {
        self.client
            .request("state_getRuntimeVersion", rpc_params![])
            .await
            .context("failed to fetch runtime version")
    }

    pub async fn genesis_hash(&self) -> anyhow::Result<H256> {
        self.client
            .request("chain_getBlockHash", rpc_params![0u32])
            .await
            .context("failed to fetch genesis hash")
    }

    pub async fn importer_account(&self) -> anyhow::Result<Option<ialp_runtime::AccountId>> {
        self.load_storage_value(importer_account_storage_key())
            .await
    }

    pub async fn observed_import(
        &self,
        export_id: ExportId,
    ) -> anyhow::Result<Option<ObservedImportRecord>> {
        self.load_storage_value(observed_import_storage_key(export_id))
            .await
    }

    pub async fn account_next_index(
        &self,
        account: &ialp_runtime::AccountId,
    ) -> anyhow::Result<u32> {
        let ss58 = account.to_ss58check();
        self.client
            .request("system_accountNextIndex", rpc_params![ss58])
            .await
            .context("failed to fetch account next index")
    }

    pub async fn submit_extrinsic(&self, extrinsic: Vec<u8>) -> anyhow::Result<String> {
        let encoded = format!("0x{}", hex::encode(extrinsic));
        self.client
            .request("author_submitExtrinsic", rpc_params![encoded])
            .await
            .context("failed to submit extrinsic")
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

pub fn load_submitter_pair(suri: &str) -> anyhow::Result<Sr25519Pair> {
    Sr25519Pair::from_string(suri, None)
        .map_err(|error| anyhow!("failed to load submitter pair from SURI '{suri}': {error}"))
}

fn decode_hex_bytes(value: &str) -> anyhow::Result<Vec<u8>> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(trimmed).with_context(|| format!("failed to decode hex bytes '{value}'"))
}
