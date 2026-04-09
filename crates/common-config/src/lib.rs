use std::{
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::Context;
use ialp_common_types::{
    fixed_bytes, ChainIdentity, DomainId, CHAIN_ID_BYTES, CHAIN_NAME_BYTES, TOKEN_SYMBOL_BYTES,
};
use multiaddr::{Multiaddr, Protocol};
use serde::Deserialize;
use thiserror::Error;

const DEFAULT_CONFIG_ROOT: &str = "config/domains";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    ReadFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    ParseFailed {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("{0}")]
    Validation(String),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DomainChainType {
    Development,
    Local,
    Live,
}

impl fmt::Display for DomainChainType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Development => "development",
            Self::Local => "local",
            Self::Live => "live",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct TokenConfig {
    pub symbol: String,
    pub decimals: u8,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct NetworkConfig {
    pub p2p_port: u16,
    pub rpc_port: u16,
    pub prometheus_port: u16,
    #[serde(default)]
    pub bootnodes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct EpochConfig {
    pub length_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct AuthorityConfig {
    pub name: String,
    pub account_seed: String,
    pub aura_seed: String,
    pub grandpa_seed: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct BootstrapConfig {
    pub sudo_account_seed: String,
    pub endowed_accounts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct DomainConfig {
    pub domain_id: DomainId,
    pub chain_name: String,
    pub chain_id: String,
    pub chain_type: DomainChainType,
    pub token: TokenConfig,
    pub network: NetworkConfig,
    pub epoch: EpochConfig,
    pub authorities: Vec<AuthorityConfig>,
    pub bootstrap: BootstrapConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedDomainConfig {
    pub source: PathBuf,
    pub config: DomainConfig,
}

impl DomainConfig {
    pub fn validate(&self, expected_domain: DomainId) -> Result<(), ConfigError> {
        if self.domain_id != expected_domain {
            return Err(ConfigError::Validation(format!(
                "config domain_id {} does not match requested domain {}",
                self.domain_id, expected_domain
            )));
        }
        if self.chain_name.trim().is_empty() {
            return Err(ConfigError::Validation(
                "chain_name must not be empty".into(),
            ));
        }
        if self.chain_name.len() > CHAIN_NAME_BYTES {
            return Err(ConfigError::Validation(format!(
                "chain_name exceeds {} bytes",
                CHAIN_NAME_BYTES
            )));
        }
        if self.chain_id.trim().is_empty() {
            return Err(ConfigError::Validation("chain_id must not be empty".into()));
        }
        if self.chain_id.len() > CHAIN_ID_BYTES {
            return Err(ConfigError::Validation(format!(
                "chain_id exceeds {} bytes",
                CHAIN_ID_BYTES
            )));
        }
        if self.token.symbol.trim().is_empty() {
            return Err(ConfigError::Validation(
                "token symbol must not be empty".into(),
            ));
        }
        if self.token.symbol.len() > TOKEN_SYMBOL_BYTES {
            return Err(ConfigError::Validation(format!(
                "token symbol exceeds {} bytes",
                TOKEN_SYMBOL_BYTES
            )));
        }
        if self.epoch.length_seconds == 0 {
            return Err(ConfigError::Validation(
                "epoch.length_seconds must be greater than zero".into(),
            ));
        }
        if self.authorities.is_empty() {
            return Err(ConfigError::Validation(
                "at least one authority must be configured".into(),
            ));
        }
        if self.bootstrap.endowed_accounts.is_empty() {
            return Err(ConfigError::Validation(
                "at least one endowed account must be configured".into(),
            ));
        }
        let ports = [
            self.network.p2p_port,
            self.network.rpc_port,
            self.network.prometheus_port,
        ];
        if ports.contains(&0) {
            return Err(ConfigError::Validation(
                "network ports must be non-zero".into(),
            ));
        }
        if self.network.p2p_port == self.network.rpc_port
            || self.network.p2p_port == self.network.prometheus_port
            || self.network.rpc_port == self.network.prometheus_port
        {
            return Err(ConfigError::Validation(
                "network ports must not overlap".into(),
            ));
        }

        for authority in &self.authorities {
            if authority.name.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "authority name must not be empty".into(),
                ));
            }
            for (label, value) in [
                ("account_seed", authority.account_seed.as_str()),
                ("aura_seed", authority.aura_seed.as_str()),
                ("grandpa_seed", authority.grandpa_seed.as_str()),
            ] {
                if value.trim().is_empty() {
                    return Err(ConfigError::Validation(format!(
                        "authority {} must not be empty",
                        label
                    )));
                }
            }
        }

        for seed in &self.bootstrap.endowed_accounts {
            if seed.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "endowed account seed must not be empty".into(),
                ));
            }
        }

        if self.bootstrap.sudo_account_seed.trim().is_empty() {
            return Err(ConfigError::Validation(
                "bootstrap sudo account seed must not be empty".into(),
            ));
        }

        for bootnode in &self.network.bootnodes {
            let parsed = Multiaddr::from_str(bootnode).map_err(|error| {
                ConfigError::Validation(format!("bootnode '{bootnode}' is invalid: {error}"))
            })?;
            if !parsed
                .iter()
                .any(|protocol| matches!(protocol, Protocol::P2p(_)))
            {
                return Err(ConfigError::Validation(format!(
                    "bootnode '{bootnode}' must include a /p2p component"
                )));
            }
        }

        Ok(())
    }

    pub fn epoch_length_blocks(&self, millis_per_block: u64) -> Result<u32, ConfigError> {
        let millis = self
            .epoch
            .length_seconds
            .checked_mul(1_000)
            .ok_or_else(|| {
                ConfigError::Validation("epoch length overflowed milliseconds".into())
            })?;
        let blocks = millis / millis_per_block;
        if blocks == 0 {
            return Err(ConfigError::Validation(
                "epoch length must be at least one block".into(),
            ));
        }
        u32::try_from(blocks)
            .map_err(|_| ConfigError::Validation("epoch length does not fit into u32".into()))
    }
}

pub fn load_domain_config(
    domain: DomainId,
    path_override: Option<&Path>,
) -> Result<LoadedDomainConfig, ConfigError> {
    let path = path_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_config_path(domain));
    let contents = fs::read_to_string(&path).map_err(|source| ConfigError::ReadFailed {
        path: path.clone(),
        source,
    })?;
    let config: DomainConfig =
        toml::from_str(&contents).map_err(|source| ConfigError::ParseFailed {
            path: path.clone(),
            source,
        })?;
    config.validate(domain)?;
    Ok(LoadedDomainConfig {
        source: path,
        config,
    })
}

pub fn build_chain_identity(config: &DomainConfig) -> ChainIdentity {
    ChainIdentity {
        domain_id: config.domain_id,
        chain_id: fixed_bytes(config.chain_id.as_bytes()),
        chain_name: fixed_bytes(config.chain_name.as_bytes()),
        token_symbol: fixed_bytes(config.token.symbol.as_bytes()),
        token_decimals: config.token.decimals,
    }
}

pub fn default_config_path(domain: DomainId) -> PathBuf {
    PathBuf::from(DEFAULT_CONFIG_ROOT).join(format!("{domain}.toml"))
}

pub fn load_workspace_domain_config(domain: DomainId) -> anyhow::Result<LoadedDomainConfig> {
    load_domain_config(domain, None)
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!(
                "failed to load workspace config for domain {}",
                domain.as_str()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root to exist")
    }

    #[test]
    fn loads_earth_config() {
        let config_path = workspace_root().join("config/domains/earth.toml");
        let loaded = load_domain_config(DomainId::Earth, Some(&config_path)).expect("config loads");

        assert_eq!(loaded.config.domain_id, DomainId::Earth);
        assert_eq!(loaded.config.chain_id, "ialp-earth-local");
        assert_eq!(loaded.config.epoch.length_seconds, 1_800);
    }

    #[test]
    fn build_chain_identity_uses_config_values() {
        let config_path = workspace_root().join("config/domains/moon.toml");
        let loaded = load_domain_config(DomainId::Moon, Some(&config_path)).expect("config loads");
        let identity = build_chain_identity(&loaded.config);

        assert_eq!(identity.domain_id, DomainId::Moon);
        assert_eq!(&identity.chain_id[..15], b"ialp-moon-local");
        assert_eq!(&identity.token_symbol[..4], b"IALP");
    }

    #[test]
    fn loads_all_workspace_domain_configs() {
        for domain in [DomainId::Earth, DomainId::Moon, DomainId::Mars] {
            let config_path = workspace_root().join(default_config_path(domain));
            let loaded =
                load_domain_config(domain, Some(&config_path)).expect("workspace config loads");
            assert_eq!(loaded.config.domain_id, domain);
        }
    }
}
