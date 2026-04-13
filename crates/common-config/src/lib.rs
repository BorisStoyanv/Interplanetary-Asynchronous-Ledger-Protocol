use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::Context;
use chrono::{DateTime, Utc};
use ialp_common_types::{
    fixed_bytes, ChainIdentity, DomainId, CHAIN_ID_BYTES, CHAIN_NAME_BYTES, TOKEN_SYMBOL_BYTES,
};
use multiaddr::{Multiaddr, Protocol};
use serde::Deserialize;
use thiserror::Error;

const DEFAULT_CONFIG_ROOT: &str = "config/domains";
const DEFAULT_TRANSPORT_CONFIG_PATH: &str = "config/transport/local.toml";

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
    pub importer_account_seed: String,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RelayTransportConfig {
    pub listen_addr: String,
    pub store_dir: PathBuf,
    pub scheduler_tick_millis: u64,
    pub ack_poll_millis: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ImporterTransportConfig {
    pub listen_addr: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct BlackoutWindowConfig {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct LinkProfileConfig {
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub base_one_way_delay_seconds: u64,
    pub initial_retry_delay_seconds: u64,
    pub max_retry_delay_seconds: u64,
    pub max_attempts: u32,
    #[serde(default)]
    pub blackout_windows: Vec<BlackoutWindowConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct TransportConfig {
    pub relay: RelayTransportConfig,
    pub importers: BTreeMap<DomainId, ImporterTransportConfig>,
    pub links: Vec<LinkProfileConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedTransportConfig {
    pub source: PathBuf,
    pub config: TransportConfig,
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
        if self.bootstrap.importer_account_seed.trim().is_empty() {
            return Err(ConfigError::Validation(
                "bootstrap importer account seed must not be empty".into(),
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

impl TransportConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.relay.listen_addr.trim().is_empty() {
            return Err(ConfigError::Validation(
                "relay.listen_addr must not be empty".into(),
            ));
        }
        if self.relay.store_dir.as_os_str().is_empty() {
            return Err(ConfigError::Validation(
                "relay.store_dir must not be empty".into(),
            ));
        }
        if self.relay.scheduler_tick_millis == 0 {
            return Err(ConfigError::Validation(
                "relay.scheduler_tick_millis must be greater than zero".into(),
            ));
        }
        if self.relay.ack_poll_millis == 0 {
            return Err(ConfigError::Validation(
                "relay.ack_poll_millis must be greater than zero".into(),
            ));
        }

        let expected_domains = [DomainId::Earth, DomainId::Moon, DomainId::Mars];
        for domain in expected_domains {
            let importer = self.importers.get(&domain).ok_or_else(|| {
                ConfigError::Validation(format!(
                    "missing importer.listen_addr for domain {}",
                    domain.as_str()
                ))
            })?;
            if importer.listen_addr.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "importer.listen_addr for domain {} must not be empty",
                    domain.as_str()
                )));
            }
        }

        let mut importer_addresses = self
            .importers
            .values()
            .map(|config| config.listen_addr.as_str())
            .collect::<Vec<_>>();
        importer_addresses.sort_unstable();
        importer_addresses.dedup();
        if importer_addresses.len() != self.importers.len() {
            return Err(ConfigError::Validation(
                "importer.listen_addr values must be unique".into(),
            ));
        }

        if self.links.len() != 6 {
            return Err(ConfigError::Validation(
                "transport config must declare exactly 6 directed links".into(),
            ));
        }

        let mut seen_links = BTreeMap::<(DomainId, DomainId), ()>::new();
        for link in &self.links {
            if link.source_domain == link.target_domain {
                return Err(ConfigError::Validation(format!(
                    "link {} -> {} must not target the same domain",
                    link.source_domain, link.target_domain
                )));
            }
            if link.base_one_way_delay_seconds == 0 {
                return Err(ConfigError::Validation(format!(
                    "link {} -> {} base_one_way_delay_seconds must be greater than zero",
                    link.source_domain, link.target_domain
                )));
            }
            if link.initial_retry_delay_seconds == 0 {
                return Err(ConfigError::Validation(format!(
                    "link {} -> {} initial_retry_delay_seconds must be greater than zero",
                    link.source_domain, link.target_domain
                )));
            }
            if link.max_retry_delay_seconds < link.initial_retry_delay_seconds {
                return Err(ConfigError::Validation(format!(
                    "link {} -> {} max_retry_delay_seconds must be >= initial_retry_delay_seconds",
                    link.source_domain, link.target_domain
                )));
            }
            for window in &link.blackout_windows {
                if window.start >= window.end {
                    return Err(ConfigError::Validation(format!(
                        "link {} -> {} blackout window start must be before end",
                        link.source_domain, link.target_domain
                    )));
                }
            }

            if seen_links
                .insert((link.source_domain, link.target_domain), ())
                .is_some()
            {
                return Err(ConfigError::Validation(format!(
                    "duplicate directed link {} -> {}",
                    link.source_domain, link.target_domain
                )));
            }
        }

        for source in expected_domains {
            for target in expected_domains {
                if source == target {
                    continue;
                }
                if !seen_links.contains_key(&(source, target)) {
                    return Err(ConfigError::Validation(format!(
                        "missing directed link {} -> {}",
                        source, target
                    )));
                }
            }
        }

        Ok(())
    }

    pub fn link(
        &self,
        source_domain: DomainId,
        target_domain: DomainId,
    ) -> Option<&LinkProfileConfig> {
        self.links
            .iter()
            .find(|link| link.source_domain == source_domain && link.target_domain == target_domain)
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

pub fn load_transport_config(
    path_override: Option<&Path>,
) -> Result<LoadedTransportConfig, ConfigError> {
    let path = path_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_transport_config_path);
    let contents = fs::read_to_string(&path).map_err(|source| ConfigError::ReadFailed {
        path: path.clone(),
        source,
    })?;
    let config: TransportConfig =
        toml::from_str(&contents).map_err(|source| ConfigError::ParseFailed {
            path: path.clone(),
            source,
        })?;
    config.validate()?;
    Ok(LoadedTransportConfig {
        source: path,
        config,
    })
}

pub fn default_config_path(domain: DomainId) -> PathBuf {
    PathBuf::from(DEFAULT_CONFIG_ROOT).join(format!("{domain}.toml"))
}

pub fn default_transport_config_path() -> PathBuf {
    PathBuf::from(DEFAULT_TRANSPORT_CONFIG_PATH)
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

pub fn load_workspace_transport_config() -> anyhow::Result<LoadedTransportConfig> {
    load_transport_config(None)
        .map_err(anyhow::Error::from)
        .with_context(|| "failed to load workspace transport config")
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

    #[test]
    fn loads_workspace_transport_config() {
        let config_path = workspace_root().join(default_transport_config_path());
        let loaded = load_transport_config(Some(&config_path)).expect("transport config loads");

        assert_eq!(loaded.config.links.len(), 6);
        assert_eq!(
            loaded
                .config
                .importers
                .get(&DomainId::Mars)
                .expect("mars importer")
                .listen_addr,
            "127.0.0.1:9953"
        );
        assert!(loaded
            .config
            .link(DomainId::Earth, DomainId::Moon)
            .is_some());
    }

    #[test]
    fn rejects_missing_directed_link() {
        let config: TransportConfig = toml::from_str(
            r#"
                [relay]
                listen_addr = "127.0.0.1:9950"
                store_dir = "var/relay"
                scheduler_tick_millis = 500
                ack_poll_millis = 500

                [importers.earth]
                listen_addr = "127.0.0.1:9951"

                [importers.moon]
                listen_addr = "127.0.0.1:9952"

                [importers.mars]
                listen_addr = "127.0.0.1:9953"

                [[links]]
                source_domain = "earth"
                target_domain = "moon"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0

                [[links]]
                source_domain = "earth"
                target_domain = "mars"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0

                [[links]]
                source_domain = "moon"
                target_domain = "earth"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0

                [[links]]
                source_domain = "moon"
                target_domain = "mars"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0

                [[links]]
                source_domain = "mars"
                target_domain = "earth"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0
            "#,
        )
        .expect("toml parses");

        let error = config.validate().expect_err("validation should fail");
        assert!(error.to_string().contains("exactly 6 directed links"));
    }

    #[test]
    fn rejects_invalid_blackout_window() {
        let config: TransportConfig = toml::from_str(
            r#"
                [relay]
                listen_addr = "127.0.0.1:9950"
                store_dir = "var/relay"
                scheduler_tick_millis = 500
                ack_poll_millis = 500

                [importers.earth]
                listen_addr = "127.0.0.1:9951"

                [importers.moon]
                listen_addr = "127.0.0.1:9952"

                [importers.mars]
                listen_addr = "127.0.0.1:9953"

                [[links]]
                source_domain = "earth"
                target_domain = "moon"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0

                [[links.blackout_windows]]
                start = "2026-01-01T01:00:00Z"
                end = "2026-01-01T00:00:00Z"

                [[links]]
                source_domain = "earth"
                target_domain = "mars"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0

                [[links]]
                source_domain = "moon"
                target_domain = "earth"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0

                [[links]]
                source_domain = "moon"
                target_domain = "mars"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0

                [[links]]
                source_domain = "mars"
                target_domain = "earth"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0

                [[links]]
                source_domain = "mars"
                target_domain = "moon"
                base_one_way_delay_seconds = 1
                initial_retry_delay_seconds = 1
                max_retry_delay_seconds = 1
                max_attempts = 0
            "#,
        )
        .expect("toml parses");

        let error = config.validate().expect_err("validation should fail");
        assert!(error
            .to_string()
            .contains("blackout window start must be before end"));
    }
}
