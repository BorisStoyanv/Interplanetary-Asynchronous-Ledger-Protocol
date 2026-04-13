use anyhow::anyhow;
use codec::Encode;
use ialp_common_types::RelayPackageEnvelopeV1;
use reqwest::{Client, StatusCode};
use serde::Deserialize;

#[derive(Clone)]
pub struct RelayHttpClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelaySubmitReceipt {
    pub accepted: bool,
    pub idempotent: bool,
    pub state: String,
}

#[derive(Debug)]
pub struct RelaySubmitError {
    pub retryable: bool,
    pub message: String,
}

impl RelayHttpClient {
    pub fn new(base_url: &str) -> anyhow::Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(anyhow!(
                "relay url '{}' must start with http:// or https://",
                base_url
            ));
        }
        Ok(Self {
            client: Client::new(),
            base_url,
        })
    }

    pub async fn submit_package(
        &self,
        envelope: &RelayPackageEnvelopeV1,
    ) -> Result<RelaySubmitReceipt, RelaySubmitError> {
        let response = self
            .client
            .post(format!("{}/api/v1/packages", self.base_url))
            .header("content-type", "application/octet-stream")
            .body(envelope.encode())
            .send()
            .await
            .map_err(|error| RelaySubmitError {
                retryable: true,
                message: format!("failed to submit package to relay: {error}"),
            })?;

        let status = response.status();
        let body = response.bytes().await.map_err(|error| RelaySubmitError {
            retryable: status.is_server_error(),
            message: format!("failed to read relay response body: {error}"),
        })?;

        if status == StatusCode::OK || status == StatusCode::ACCEPTED {
            return serde_json::from_slice::<RelaySubmitReceipt>(&body).map_err(|error| {
                RelaySubmitError {
                    retryable: false,
                    message: format!("failed to decode relay receipt: {error}"),
                }
            });
        }

        let message = std::str::from_utf8(&body)
            .map(|value| value.to_string())
            .unwrap_or_else(|_| format!("non-utf8 relay response body: 0x{}", hex::encode(&body)));
        Err(RelaySubmitError {
            retryable: status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS,
            message: format!("relay rejected package with status {}: {}", status, message),
        })
    }
}

pub fn ensure_submission_succeeded(receipt: &RelaySubmitReceipt) -> anyhow::Result<()> {
    if !receipt.accepted {
        return Err(anyhow!(
            "relay returned non-accepted receipt in state {}",
            receipt.state
        ));
    }
    Ok(())
}
