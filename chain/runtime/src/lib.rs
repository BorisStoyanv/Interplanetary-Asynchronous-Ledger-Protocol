#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

pub mod apis;
pub mod genesis_config_presets;

extern crate alloc;

use alloc::vec::Vec;
use frame_support::{
    derive_impl, parameter_types,
    traits::{ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, VariantCountOf},
    weights::{
        constants::{RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
        IdentityFee, Weight,
    },
};
use frame_system::limits::{BlockLength, BlockWeights};
use ialp_common_types::validator_set_hash as compute_validator_set_hash;
use pallet_transaction_payment::{ConstFeeMultiplier, FungibleAdapter, Multiplier};
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;
use sp_runtime::{
    generic, impl_opaque_keys,
    traits::{BlakeTwo256, IdentifyAccount, One, Verify},
    MultiAddress, MultiSignature, Perbill,
};
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

pub mod opaque {
    use super::*;
    use sp_runtime::{
        generic,
        traits::{BlakeTwo256, Hash as HashT},
    };

    pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;

    pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
    pub type Block = generic::Block<Header, UncheckedExtrinsic>;
    pub type BlockId = generic::BlockId<Block>;
    pub type Hash = <BlakeTwo256 as HashT>::Output;
}

impl_opaque_keys! {
    pub struct SessionKeys {
        pub aura: Aura,
        pub grandpa: Grandpa,
    }
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: alloc::borrow::Cow::Borrowed("ialp-runtime"),
    impl_name: alloc::borrow::Cow::Borrowed("ialp-runtime"),
    authoring_version: 1,
    spec_version: 1,
    impl_version: 1,
    apis: apis::RUNTIME_API_VERSIONS,
    transaction_version: 1,
    system_version: 1,
};

pub const MILLI_SECS_PER_BLOCK: u64 = 6_000;
pub const SLOT_DURATION: u64 = MILLI_SECS_PER_BLOCK;
pub const EXISTENTIAL_DEPOSIT: Balance = 1_000_000_000;
pub const BLOCK_HASH_COUNT: BlockNumber = 2_400;

#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
    NativeVersion {
        runtime_version: VERSION,
        can_author_with: Default::default(),
    }
}

pub type Signature = MultiSignature;
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;
pub type Balance = u128;
pub type Nonce = u32;
pub type Hash = sp_core::H256;
pub type BlockNumber = u32;
pub type Address = MultiAddress<AccountId, ()>;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
pub type SignedBlock = generic::SignedBlock<Block>;
pub type BlockId = generic::BlockId<Block>;

pub type TxExtension = (
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckEra<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
    frame_system::WeightReclaim<Runtime>,
);

pub type UncheckedExtrinsic =
    generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, TxExtension>;
pub type SignedPayload = generic::SignedPayload<RuntimeCall, TxExtension>;

type Migrations = ();

pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
    Migrations,
>;

#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask
    )]
    pub struct Runtime;

    #[runtime::pallet_index(0)]
    pub type System = frame_system;

    #[runtime::pallet_index(1)]
    pub type Timestamp = pallet_timestamp;

    #[runtime::pallet_index(2)]
    pub type Aura = pallet_aura;

    #[runtime::pallet_index(3)]
    pub type Grandpa = pallet_grandpa;

    #[runtime::pallet_index(4)]
    pub type Balances = pallet_balances;

    #[runtime::pallet_index(5)]
    pub type TransactionPayment = pallet_transaction_payment;

    #[runtime::pallet_index(6)]
    pub type Sudo = pallet_sudo;

    #[runtime::pallet_index(7)]
    pub type Domain = pallet_ialp_domain;

    #[runtime::pallet_index(8)]
    pub type Epochs = pallet_ialp_epochs;

    #[runtime::pallet_index(9)]
    pub type Transfers = pallet_ialp_transfers;

    #[runtime::pallet_index(10)]
    pub type Governance = pallet_ialp_governance;
}

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

parameter_types! {
    pub const BlockHashCount: BlockNumber = BLOCK_HASH_COUNT;
    pub const Version: RuntimeVersion = VERSION;
    pub RuntimeBlockWeights: BlockWeights = BlockWeights::with_sensible_defaults(
        Weight::from_parts(2u64 * WEIGHT_REF_TIME_PER_SECOND, u64::MAX),
        NORMAL_DISPATCH_RATIO,
    );
    pub RuntimeBlockLength: BlockLength = BlockLength::max_with_normal_ratio(
        5 * 1024 * 1024,
        NORMAL_DISPATCH_RATIO,
    );
    pub const SS58Prefix: u8 = 42;
}

#[derive_impl(frame_system::config_preludes::SolochainDefaultConfig)]
impl frame_system::Config for Runtime {
    type Block = Block;
    type BlockWeights = RuntimeBlockWeights;
    type BlockLength = RuntimeBlockLength;
    type AccountId = AccountId;
    type Nonce = Nonce;
    type Hash = Hash;
    type BlockHashCount = BlockHashCount;
    type DbWeight = RocksDbWeight;
    type Version = Version;
    type AccountData = pallet_balances::AccountData<Balance>;
    type SS58Prefix = SS58Prefix;
    type MaxConsumers = frame_support::traits::ConstU32<16>;
}

impl pallet_aura::Config for Runtime {
    type AuthorityId = AuraId;
    type DisabledValidators = ();
    type MaxAuthorities = ConstU32<32>;
    type AllowMultipleBlocksPerSlot = ConstBool<false>;
    type SlotDuration = pallet_aura::MinimumPeriodTimesTwo<Runtime>;
}

impl pallet_grandpa::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type MaxAuthorities = ConstU32<32>;
    type MaxNominators = ConstU32<0>;
    type MaxSetIdSessionEntries = ConstU64<0>;
    type KeyOwnerProof = sp_core::Void;
    type EquivocationReportSystem = ();
}

impl pallet_timestamp::Config for Runtime {
    type Moment = u64;
    type OnTimestampSet = Aura;
    type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
    type WeightInfo = ();
}

impl pallet_balances::Config for Runtime {
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ();
    type ReserveIdentifier = [u8; 8];
    type Balance = Balance;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ConstU128<EXISTENTIAL_DEPOSIT>;
    type AccountStore = System;
    type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
    type FreezeIdentifier = RuntimeFreezeReason;
    type MaxFreezes = VariantCountOf<RuntimeFreezeReason>;
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type DoneSlashHandler = ();
}

parameter_types! {
    pub FeeMultiplier: Multiplier = Multiplier::one();
}

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnChargeTransaction = FungibleAdapter<Balances, ()>;
    type OperationalFeeMultiplier = ConstU8<5>;
    type WeightToFee = IdentityFee<Balance>;
    type LengthToFee = IdentityFee<Balance>;
    type FeeMultiplierUpdate = ConstFeeMultiplier<FeeMultiplier>;
    type WeightInfo = pallet_transaction_payment::weights::SubstrateWeight<Runtime>;
}

impl pallet_sudo::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type WeightInfo = pallet_sudo::weights::SubstrateWeight<Runtime>;
}

impl pallet_ialp_domain::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
}

pub struct EpochSummaryRuntimeContext;

impl pallet_ialp_epochs::SummaryContext<Hash> for EpochSummaryRuntimeContext {
    fn domain_id() -> ialp_common_types::DomainId {
        Domain::domain_id()
    }

    fn validator_set_hash() -> [u8; 32] {
        // The runtime adapter owns pallet coupling so `pallet-epochs` depends only on the narrow
        // summary context surface it needs for canonical header authoring.
        compute_validator_set_hash(Grandpa::current_set_id(), &Grandpa::grandpa_authorities())
    }

    fn hash_to_bytes(hash: &Hash) -> [u8; 32] {
        hash.to_fixed_bytes()
    }
}

pub struct EpochExportCommitmentRuntimeProvider;

impl pallet_ialp_epochs::ExportCommitmentProvider for EpochExportCommitmentRuntimeProvider {
    fn commit_epoch_exports(
        epoch_id: ialp_common_types::EpochId,
        start_block_height: u32,
        end_block_height: u32,
    ) -> [u8; 32] {
        Transfers::commit_epoch_exports(epoch_id, start_block_height, end_block_height)
    }
}

pub struct EpochImportCommitmentRuntimeProvider;

impl pallet_ialp_epochs::ImportCommitmentProvider for EpochImportCommitmentRuntimeProvider {
    fn commit_epoch_imports(
        epoch_id: ialp_common_types::EpochId,
        start_block_height: u32,
        end_block_height: u32,
    ) -> [u8; 32] {
        Transfers::commit_epoch_imports(epoch_id, start_block_height, end_block_height)
    }
}

pub struct EpochGovernanceCommitmentRuntimeProvider;

impl pallet_ialp_epochs::GovernanceCommitmentProvider
    for EpochGovernanceCommitmentRuntimeProvider
{
    fn commit_epoch_governance(
        epoch_id: ialp_common_types::EpochId,
        start_block_height: u32,
        end_block_height: u32,
    ) -> [u8; 32] {
        Governance::commit_epoch_governance(epoch_id, start_block_height, end_block_height)
    }
}

impl pallet_ialp_epochs::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type SummaryContext = EpochSummaryRuntimeContext;
    type ExportCommitmentProvider = EpochExportCommitmentRuntimeProvider;
    type ImportCommitmentProvider = EpochImportCommitmentRuntimeProvider;
    type GovernanceCommitmentProvider = EpochGovernanceCommitmentRuntimeProvider;
}

pub struct TransferDomainIdentityRuntimeProvider;

impl pallet_ialp_transfers::DomainIdentityProvider for TransferDomainIdentityRuntimeProvider {
    fn domain_id() -> ialp_common_types::DomainId {
        Domain::domain_id()
    }
}

pub struct TransferEpochInfoRuntimeProvider;

impl pallet_ialp_transfers::EpochInfoProvider for TransferEpochInfoRuntimeProvider {
    fn current_epoch() -> ialp_common_types::EpochId {
        Epochs::current_epoch()
    }
}

impl pallet_ialp_transfers::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type RuntimeHoldReason = RuntimeHoldReason;
    type DomainIdentity = TransferDomainIdentityRuntimeProvider;
    type EpochInfo = TransferEpochInfoRuntimeProvider;
}

pub struct GovernanceDomainIdentityRuntimeProvider;

impl pallet_ialp_governance::DomainIdentityProvider for GovernanceDomainIdentityRuntimeProvider {
    fn domain_id() -> ialp_common_types::DomainId {
        Domain::domain_id()
    }
}

pub struct GovernanceEpochInfoRuntimeProvider;

impl pallet_ialp_governance::EpochInfoProvider for GovernanceEpochInfoRuntimeProvider {
    fn current_epoch() -> ialp_common_types::EpochId {
        Epochs::current_epoch()
    }
}

impl pallet_ialp_governance::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type DomainIdentity = GovernanceDomainIdentityRuntimeProvider;
    type EpochInfo = GovernanceEpochInfoRuntimeProvider;
}
