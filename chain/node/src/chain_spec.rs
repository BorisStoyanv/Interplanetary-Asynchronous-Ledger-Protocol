use std::path::Path;

use frame_support::build_struct_json_patch;
use ialp_common_config::{
    build_chain_identity, load_domain_config, DomainChainType, LoadedDomainConfig,
};
use ialp_common_types::DomainId;
use sc_service::{ChainType, Properties};
use serde_json::Value;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_consensus_grandpa::AuthorityId as GrandpaId;
use sp_core::{sr25519, Pair, Public};
use sp_runtime::traits::{IdentifyAccount, Verify};

pub type ChainSpec = sc_service::GenericChainSpec;

type AccountPublic = <ialp_runtime::Signature as Verify>::Signer;

// Phase 0 keeps the chain-spec builder domain-aware because Earth/Moon/Mars are
// first-class protocol identities, not a cosmetic rename over one dev chain.
pub fn load_domain_chain_spec(
    domain: DomainId,
    path_override: Option<&Path>,
) -> Result<ChainSpec, String> {
    let loaded = load_domain_config(domain, path_override).map_err(|error| error.to_string())?;
    let mut properties = Properties::new();
    properties.insert(
        "tokenDecimals".to_string(),
        loaded.config.token.decimals.into(),
    );
    properties.insert(
        "tokenSymbol".to_string(),
        loaded.config.token.symbol.clone().into(),
    );
    properties.insert(
        "domainId".to_string(),
        loaded.config.domain_id.to_string().into(),
    );

    let spec = ChainSpec::builder(
        ialp_runtime::WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?,
        None,
    )
    .with_name(&loaded.config.chain_name)
    .with_id(&loaded.config.chain_id)
    .with_chain_type(chain_type(&loaded.config.chain_type))
    .with_properties(properties)
    .with_genesis_config_patch(genesis_patch(&loaded)?)
    .build();

    Ok(spec)
}

pub fn domain_config_for_cli(
    domain: DomainId,
    path_override: Option<&Path>,
) -> Result<LoadedDomainConfig, sc_cli::Error> {
    load_domain_config(domain, path_override)
        .map_err(|error| sc_cli::Error::Input(format!("config validation failed: {error}")))
}

fn chain_type(chain_type: &DomainChainType) -> ChainType {
    match chain_type {
        DomainChainType::Development => ChainType::Development,
        DomainChainType::Local => ChainType::Local,
        DomainChainType::Live => ChainType::Live,
    }
}

fn genesis_patch(loaded: &LoadedDomainConfig) -> Result<Value, String> {
    let epoch_length_blocks = loaded
        .config
        .epoch_length_blocks(ialp_runtime::MILLI_SECS_PER_BLOCK)
        .map_err(|error| error.to_string())?;
    let chain_identity = build_chain_identity(&loaded.config);

    let initial_authorities = loaded
        .config
        .authorities
        .iter()
        .map(|authority| {
            (
                get_from_seed::<AuraId>(&authority.aura_seed),
                get_from_seed::<GrandpaId>(&authority.grandpa_seed),
            )
        })
        .collect::<Vec<_>>();

    let root_key = get_account_id_from_seed(&loaded.config.bootstrap.sudo_account_seed);

    let mut endowed_accounts = loaded
        .config
        .bootstrap
        .endowed_accounts
        .iter()
        .map(|seed| (get_account_id_from_seed(seed), 1u128 << 60))
        .collect::<Vec<_>>();

    for authority in &loaded.config.authorities {
        let authority_account = get_account_id_from_seed(&authority.account_seed);
        if !endowed_accounts
            .iter()
            .any(|(account, _)| account == &authority_account)
        {
            endowed_accounts.push((authority_account, 1u128 << 60));
        }
    }

    Ok(build_struct_json_patch!(
        ialp_runtime::RuntimeGenesisConfig {
            balances: ialp_runtime::BalancesConfig {
                balances: endowed_accounts,
            },
            aura: pallet_aura::GenesisConfig {
                authorities: initial_authorities
                    .iter()
                    .map(|(aura, _)| aura.clone())
                    .collect::<Vec<_>>(),
            },
            grandpa: pallet_grandpa::GenesisConfig {
                authorities: initial_authorities
                    .iter()
                    .map(|(_, grandpa)| (grandpa.clone(), 1))
                    .collect::<Vec<_>>(),
            },
            sudo: ialp_runtime::SudoConfig {
                key: Some(root_key),
            },
            domain: pallet_ialp_domain::GenesisConfig { chain_identity },
            epochs: pallet_ialp_epochs::GenesisConfig {
                epoch_length_blocks,
            },
        }
    ))
}

fn get_account_id_from_seed(seed: &str) -> ialp_runtime::AccountId {
    AccountPublic::from(get_pair_from_seed::<sr25519::Public>(seed)).into_account()
}

fn get_pair_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public
where
    TPublic::Pair: Pair,
{
    TPublic::Pair::from_string(seed, None)
        .expect("seed is valid for local bootstrap")
        .public()
}

fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public
where
    TPublic::Pair: Pair,
{
    get_pair_from_seed::<TPublic>(seed)
}
