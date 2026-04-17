#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{string::String, vec, vec::Vec};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::{cmp::Ordering, fmt, str::FromStr};
use scale_info::TypeInfo;
use sp_io::hashing::{blake2_128, blake2_256, twox_128};

#[cfg(feature = "serde")]
use serde::{de::Error as DeError, Deserializer, Serializer};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

pub type EpochId = u64;
pub const CHAIN_ID_BYTES: usize = 64;
pub const CHAIN_NAME_BYTES: usize = 64;
pub const TOKEN_SYMBOL_BYTES: usize = 16;
pub const EPOCH_SUMMARY_VERSION: u16 = 1;
pub const EMPTY_HASH: [u8; 32] = [0u8; 32];
pub const BLOCK_ROOT_LABEL: &str = "IALP:block-root:v1";
pub const TX_ROOT_LABEL: &str = "IALP:tx-root:v1";
pub const EVENT_ROOT_LABEL: &str = "IALP:event-root:v1";
pub const EXPORT_ROOT_EMPTY_LABEL: &str = "IALP:export-root:empty:v1";
pub const IMPORT_ROOT_EMPTY_LABEL: &str = "IALP:import-root:empty:v1";
pub const GOVERNANCE_ROOT_EMPTY_LABEL: &str = "IALP:governance-root:empty:v1";
pub const CERTIFIED_SUMMARY_PACKAGE_VERSION: u16 = 1;
pub const GRANDPA_FINALITY_CERTIFICATE_VERSION: u16 = 1;
pub const SUMMARY_HEADER_STORAGE_PROOF_VERSION: u16 = 1;
pub const EXPORT_LEAF_VERSION: u16 = 1;
pub const OBSERVED_IMPORT_VERSION: u16 = 1;
pub const EXPORT_INCLUSION_PROOF_VERSION: u16 = 1;
pub const FINALIZED_IMPORT_LEAF_VERSION: u16 = 1;
pub const FINALIZED_IMPORT_INCLUSION_PROOF_VERSION: u16 = 1;
pub const REMOTE_FINALIZATION_CLAIM_VERSION: u16 = 1;
pub const GOVERNANCE_PAYLOAD_VERSION: u16 = 1;
pub const GOVERNANCE_PROPOSAL_VERSION: u16 = 1;
pub const GOVERNANCE_VOTE_VERSION: u16 = 1;
pub const GOVERNANCE_ACK_RECORD_VERSION: u16 = 1;
pub const GOVERNANCE_ACTIVATION_RECORD_VERSION: u16 = 1;
pub const GOVERNANCE_PROPOSAL_LEAF_VERSION: u16 = 1;
pub const GOVERNANCE_ACK_LEAF_VERSION: u16 = 1;
pub const GOVERNANCE_INCLUSION_PROOF_VERSION: u16 = 1;
pub const SUMMARY_HEADERS_PROOF_INDEX: usize = 0;
pub const EXPORT_PROOF_START_INDEX: usize = 1;
pub const EXPORT_MERKLE_NODE_LABEL: &str = "IALP:export-merkle-node:v1";
pub const EXPORT_MERKLE_EMPTY_LABEL: &str = "IALP:export-merkle-empty:v1";
pub const IMPORT_MERKLE_NODE_LABEL: &str = "IALP:import-merkle-node:v1";
pub const IMPORT_MERKLE_EMPTY_LABEL: &str = "IALP:import-merkle-empty:v1";
pub const GOVERNANCE_MERKLE_NODE_LABEL: &str = "IALP:governance-merkle-node:v1";
pub const GOVERNANCE_MERKLE_EMPTY_LABEL: &str = "IALP:governance-merkle-empty:v1";
pub const RELAY_PACKAGE_ENVELOPE_VERSION: u16 = 1;

pub type ExportId = [u8; 32];
pub type AccountIdBytes = [u8; 32];
pub type GovernanceProposalId = [u8; 32];

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum DomainId {
    #[default]
    Earth,
    Moon,
    Mars,
}

impl DomainId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Earth => "earth",
            Self::Moon => "moon",
            Self::Mars => "mars",
        }
    }
}

impl fmt::Display for DomainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DomainId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "earth" => Ok(Self::Earth),
            "moon" => Ok(Self::Moon),
            "mars" => Ok(Self::Mars),
            _ => Err("unsupported domain id"),
        }
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub struct ChainIdentity {
    pub domain_id: DomainId,
    pub chain_id: [u8; CHAIN_ID_BYTES],
    pub chain_name: [u8; CHAIN_NAME_BYTES],
    pub token_symbol: [u8; TOKEN_SYMBOL_BYTES],
    pub token_decimals: u8,
}

impl Default for ChainIdentity {
    fn default() -> Self {
        Self {
            domain_id: DomainId::Earth,
            chain_id: [0u8; CHAIN_ID_BYTES],
            chain_name: [0u8; CHAIN_NAME_BYTES],
            token_symbol: [0u8; TOKEN_SYMBOL_BYTES],
            token_decimals: 12,
        }
    }
}

impl ChainIdentity {
    #[cfg(feature = "serde")]
    fn decode_fixed_utf8<const N: usize>(value: &[u8]) -> Result<[u8; N], &'static str> {
        if value.len() > N {
            return Err("value exceeds fixed-width field capacity");
        }

        Ok(fixed_bytes(value))
    }

    #[cfg(feature = "serde")]
    fn encode_fixed_utf8<const N: usize>(value: &[u8; N]) -> Result<&str, &'static str> {
        let len = value.iter().position(|byte| *byte == 0).unwrap_or(N);
        core::str::from_utf8(&value[..len]).map_err(|_| "value contains invalid utf-8")
    }
}

#[cfg(feature = "serde")]
#[derive(Serialize, Deserialize)]
struct ChainIdentitySerde {
    domain_id: DomainId,
    chain_id: alloc::string::String,
    chain_name: alloc::string::String,
    token_symbol: alloc::string::String,
    token_decimals: u8,
}

#[cfg(feature = "serde")]
impl Serialize for ChainIdentity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ChainIdentitySerde {
            domain_id: self.domain_id,
            chain_id: Self::encode_fixed_utf8(&self.chain_id)
                .map_err(serde::ser::Error::custom)?
                .into(),
            chain_name: Self::encode_fixed_utf8(&self.chain_name)
                .map_err(serde::ser::Error::custom)?
                .into(),
            token_symbol: Self::encode_fixed_utf8(&self.token_symbol)
                .map_err(serde::ser::Error::custom)?
                .into(),
            token_decimals: self.token_decimals,
        }
        .serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for ChainIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = ChainIdentitySerde::deserialize(deserializer)?;

        Ok(Self {
            domain_id: value.domain_id,
            chain_id: Self::decode_fixed_utf8(value.chain_id.as_bytes())
                .map_err(D::Error::custom)?,
            chain_name: Self::decode_fixed_utf8(value.chain_name.as_bytes())
                .map_err(D::Error::custom)?,
            token_symbol: Self::decode_fixed_utf8(value.token_symbol.as_bytes())
                .map_err(D::Error::custom)?,
            token_decimals: value.token_decimals,
        })
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SummaryCommitment {
    pub version: u16,
    pub domain_id: DomainId,
    pub epoch_id: EpochId,
    pub prev_summary_hash: [u8; 32],
    pub start_block_height: u32,
    pub end_block_height: u32,
    pub state_root: [u8; 32],
    pub block_root: [u8; 32],
    pub tx_root: [u8; 32],
    pub event_root: [u8; 32],
    pub export_root: [u8; 32],
    pub import_root: [u8; 32],
    pub governance_root: [u8; 32],
    pub validator_set_hash: [u8; 32],
    pub summary_hash: [u8; 32],
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EpochSummaryHashInput {
    pub version: u16,
    pub domain_id: DomainId,
    pub epoch_id: EpochId,
    pub prev_summary_hash: [u8; 32],
    pub start_block_height: u32,
    pub end_block_height: u32,
    pub state_root: [u8; 32],
    pub block_root: [u8; 32],
    pub tx_root: [u8; 32],
    pub event_root: [u8; 32],
    pub export_root: [u8; 32],
    pub import_root: [u8; 32],
    pub governance_root: [u8; 32],
    pub validator_set_hash: [u8; 32],
}

impl EpochSummaryHashInput {
    pub fn summary_hash(&self) -> [u8; 32] {
        blake2_256(&self.encode())
    }
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EpochSummaryHeader {
    pub version: u16,
    pub domain_id: DomainId,
    pub epoch_id: EpochId,
    pub prev_summary_hash: [u8; 32],
    pub start_block_height: u32,
    pub end_block_height: u32,
    pub state_root: [u8; 32],
    pub block_root: [u8; 32],
    pub tx_root: [u8; 32],
    pub event_root: [u8; 32],
    pub export_root: [u8; 32],
    pub import_root: [u8; 32],
    pub governance_root: [u8; 32],
    pub validator_set_hash: [u8; 32],
    pub summary_hash: [u8; 32],
}

impl EpochSummaryHeader {
    pub fn from_hash_input(input: EpochSummaryHashInput) -> Self {
        Self {
            version: input.version,
            domain_id: input.domain_id,
            epoch_id: input.epoch_id,
            prev_summary_hash: input.prev_summary_hash,
            start_block_height: input.start_block_height,
            end_block_height: input.end_block_height,
            state_root: input.state_root,
            block_root: input.block_root,
            tx_root: input.tx_root,
            event_root: input.event_root,
            export_root: input.export_root,
            import_root: input.import_root,
            governance_root: input.governance_root,
            validator_set_hash: input.validator_set_hash,
            summary_hash: input.summary_hash(),
        }
    }

    pub fn hash_input(&self) -> EpochSummaryHashInput {
        EpochSummaryHashInput {
            version: self.version,
            domain_id: self.domain_id,
            epoch_id: self.epoch_id,
            prev_summary_hash: self.prev_summary_hash,
            start_block_height: self.start_block_height,
            end_block_height: self.end_block_height,
            state_root: self.state_root,
            block_root: self.block_root,
            tx_root: self.tx_root,
            event_root: self.event_root,
            export_root: self.export_root,
            import_root: self.import_root,
            governance_root: self.governance_root,
            validator_set_hash: self.validator_set_hash,
        }
    }

    pub fn compute_summary_hash(&self) -> [u8; 32] {
        self.hash_input().summary_hash()
    }

    pub fn commitment(&self) -> SummaryCommitment {
        SummaryCommitment {
            version: self.version,
            domain_id: self.domain_id,
            epoch_id: self.epoch_id,
            prev_summary_hash: self.prev_summary_hash,
            start_block_height: self.start_block_height,
            end_block_height: self.end_block_height,
            state_root: self.state_root,
            block_root: self.block_root,
            tx_root: self.tx_root,
            event_root: self.event_root,
            export_root: self.export_root,
            import_root: self.import_root,
            governance_root: self.governance_root,
            validator_set_hash: self.validator_set_hash,
            summary_hash: self.summary_hash,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct StagedSummaryRecord {
    pub header: EpochSummaryHeader,
    pub staged_at_block_number: u32,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ExportStatus {
    #[default]
    LocalFinal,
    Exported,
    RemoteFinalized,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ImportObservationStatus {
    #[default]
    RemoteObserved,
    RemoteFinalized,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ExportLeafHashInput {
    pub version: u16,
    pub export_id: ExportId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub sender: AccountIdBytes,
    pub recipient: AccountIdBytes,
    pub amount: u128,
    pub source_epoch_id: EpochId,
    pub source_block_height: u32,
    pub extrinsic_index: u32,
}

impl ExportLeafHashInput {
    pub fn export_hash(&self) -> [u8; 32] {
        blake2_256(&self.encode())
    }
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ExportLeaf {
    pub version: u16,
    pub export_id: ExportId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub sender: AccountIdBytes,
    pub recipient: AccountIdBytes,
    pub amount: u128,
    pub source_epoch_id: EpochId,
    pub source_block_height: u32,
    pub extrinsic_index: u32,
    pub export_hash: [u8; 32],
}

impl ExportLeaf {
    pub fn from_hash_input(input: ExportLeafHashInput) -> Self {
        Self {
            version: input.version,
            export_id: input.export_id,
            source_domain: input.source_domain,
            target_domain: input.target_domain,
            sender: input.sender,
            recipient: input.recipient,
            amount: input.amount,
            source_epoch_id: input.source_epoch_id,
            source_block_height: input.source_block_height,
            extrinsic_index: input.extrinsic_index,
            export_hash: input.export_hash(),
        }
    }

    pub fn hash_input(&self) -> ExportLeafHashInput {
        ExportLeafHashInput {
            version: self.version,
            export_id: self.export_id,
            source_domain: self.source_domain,
            target_domain: self.target_domain,
            sender: self.sender,
            recipient: self.recipient,
            amount: self.amount,
            source_epoch_id: self.source_epoch_id,
            source_block_height: self.source_block_height,
            extrinsic_index: self.extrinsic_index,
        }
    }

    pub fn compute_export_hash(&self) -> [u8; 32] {
        self.hash_input().export_hash()
    }
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ExportRecord {
    pub leaf: ExportLeaf,
    pub status: ExportStatus,
    pub completion_summary_hash: Option<[u8; 32]>,
    pub completion_package_hash: Option<[u8; 32]>,
    pub resolved_at_source_block_height: Option<u32>,
    pub resolver_account: Option<AccountIdBytes>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ObservedImportClaim {
    pub version: u16,
    pub export_id: ExportId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub source_epoch_id: EpochId,
    pub summary_hash: [u8; 32],
    pub package_hash: [u8; 32],
    pub recipient: AccountIdBytes,
    pub amount: u128,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ObservedImportRecord {
    pub version: u16,
    pub export_id: ExportId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub source_epoch_id: EpochId,
    pub summary_hash: [u8; 32],
    pub package_hash: [u8; 32],
    pub recipient: AccountIdBytes,
    pub amount: u128,
    pub observed_at_local_block_height: u32,
    pub observer_account: AccountIdBytes,
    pub status: ImportObservationStatus,
    pub finalized_at_local_block_height: Option<u32>,
    pub finalizer_account: Option<AccountIdBytes>,
}

impl ObservedImportRecord {
    pub fn from_claim(
        claim: ObservedImportClaim,
        observed_at_local_block_height: u32,
        observer_account: AccountIdBytes,
    ) -> Self {
        Self {
            version: claim.version,
            export_id: claim.export_id,
            source_domain: claim.source_domain,
            target_domain: claim.target_domain,
            source_epoch_id: claim.source_epoch_id,
            summary_hash: claim.summary_hash,
            package_hash: claim.package_hash,
            recipient: claim.recipient,
            amount: claim.amount,
            observed_at_local_block_height,
            observer_account,
            status: ImportObservationStatus::RemoteObserved,
            finalized_at_local_block_height: None,
            finalizer_account: None,
        }
    }

    pub fn finalized_leaf(&self) -> Option<FinalizedImportLeaf> {
        if self.status != ImportObservationStatus::RemoteFinalized {
            return None;
        }

        Some(FinalizedImportLeaf::from_hash_input(
            FinalizedImportLeafHashInput {
                version: FINALIZED_IMPORT_LEAF_VERSION,
                export_id: self.export_id,
                source_domain: self.source_domain,
                target_domain: self.target_domain,
                recipient: self.recipient,
                amount: self.amount,
                source_epoch_id: self.source_epoch_id,
                summary_hash: self.summary_hash,
                package_hash: self.package_hash,
            },
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum GovernancePayload {
    SetProtocolVersion { new_version: u32 },
}

impl GovernancePayload {
    pub fn payload_hash(&self) -> [u8; 32] {
        blake2_256(&(GOVERNANCE_PAYLOAD_VERSION, self).encode())
    }

    pub fn protocol_version(&self) -> u32 {
        match self {
            Self::SetProtocolVersion { new_version } => *new_version,
        }
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum GovernanceVoteChoice {
    #[default]
    Yes,
    No,
    Abstain,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum GovernanceProposalStatus {
    #[default]
    Created,
    Voting,
    Rejected,
    LocallyFinalized,
    Scheduled,
    Activated,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GovernanceVote {
    pub version: u16,
    pub proposal_id: GovernanceProposalId,
    pub voter: AccountIdBytes,
    pub choice: GovernanceVoteChoice,
    pub voting_power: u128,
    pub cast_epoch: EpochId,
    pub cast_block_height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GovernanceProposal {
    pub version: u16,
    pub proposal_id: GovernanceProposalId,
    pub source_domain: DomainId,
    pub target_domains: Vec<DomainId>,
    pub proposer: AccountIdBytes,
    pub payload: GovernancePayload,
    pub payload_hash: [u8; 32],
    pub created_epoch: EpochId,
    pub voting_start_epoch: EpochId,
    pub voting_end_epoch: EpochId,
    pub approval_epoch: Option<EpochId>,
    pub activation_epoch: EpochId,
    pub snapshot_total_voting_power: u128,
    pub quorum_numerator: u32,
    pub quorum_denominator: u32,
    pub yes_voting_power: u128,
    pub no_voting_power: u128,
    pub abstain_voting_power: u128,
    pub status: GovernanceProposalStatus,
}

impl Default for GovernanceProposal {
    fn default() -> Self {
        Self {
            version: GOVERNANCE_PROPOSAL_VERSION,
            proposal_id: EMPTY_HASH,
            source_domain: DomainId::Earth,
            target_domains: Vec::new(),
            proposer: [0u8; 32],
            payload: GovernancePayload::SetProtocolVersion { new_version: 1 },
            payload_hash: EMPTY_HASH,
            created_epoch: 0,
            voting_start_epoch: 0,
            voting_end_epoch: 0,
            approval_epoch: None,
            activation_epoch: 0,
            snapshot_total_voting_power: 0,
            quorum_numerator: 1,
            quorum_denominator: 2,
            yes_voting_power: 0,
            no_voting_power: 0,
            abstain_voting_power: 0,
            status: GovernanceProposalStatus::Created,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GovernanceAckRecord {
    pub version: u16,
    pub proposal_id: GovernanceProposalId,
    pub source_domain: DomainId,
    pub acknowledging_domain: DomainId,
    pub target_domains: Vec<DomainId>,
    pub activation_epoch: EpochId,
    pub payload_hash: [u8; 32],
    pub new_protocol_version: u32,
    pub acknowledged_epoch: EpochId,
    pub acknowledged_at_local_block_height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GovernanceActivationRecord {
    pub version: u16,
    pub proposal_id: GovernanceProposalId,
    pub source_domain: DomainId,
    pub target_domains: Vec<DomainId>,
    pub payload_hash: [u8; 32],
    pub new_protocol_version: u32,
    pub activation_epoch: EpochId,
    pub known_ack_domains: Vec<DomainId>,
    pub scheduled_at_epoch: Option<EpochId>,
    pub activated_at_epoch: Option<EpochId>,
    pub activated_at_local_block_height: Option<u32>,
    pub status: GovernanceProposalStatus,
}

impl Default for GovernanceActivationRecord {
    fn default() -> Self {
        Self {
            version: GOVERNANCE_ACTIVATION_RECORD_VERSION,
            proposal_id: EMPTY_HASH,
            source_domain: DomainId::Earth,
            target_domains: Vec::new(),
            payload_hash: EMPTY_HASH,
            new_protocol_version: 1,
            activation_epoch: 0,
            known_ack_domains: Vec::new(),
            scheduled_at_epoch: None,
            activated_at_epoch: None,
            activated_at_local_block_height: None,
            status: GovernanceProposalStatus::LocallyFinalized,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ImportedGovernanceProposalClaim {
    pub version: u16,
    pub leaf: GovernanceProposalLeaf,
    pub summary_hash: [u8; 32],
    pub package_hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ImportedGovernanceAckClaim {
    pub version: u16,
    pub leaf: GovernanceAckLeaf,
    pub summary_hash: [u8; 32],
    pub package_hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GovernanceProposalLeafHashInput {
    pub version: u16,
    pub proposal_id: GovernanceProposalId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub target_domains: Vec<DomainId>,
    pub proposer: AccountIdBytes,
    pub payload_hash: [u8; 32],
    pub new_protocol_version: u32,
    pub created_epoch: EpochId,
    pub voting_start_epoch: EpochId,
    pub voting_end_epoch: EpochId,
    pub approval_epoch: EpochId,
    pub activation_epoch: EpochId,
}

impl GovernanceProposalLeafHashInput {
    pub fn leaf_hash(&self) -> [u8; 32] {
        blake2_256(&self.encode())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GovernanceProposalLeaf {
    pub version: u16,
    pub proposal_id: GovernanceProposalId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub target_domains: Vec<DomainId>,
    pub proposer: AccountIdBytes,
    pub payload_hash: [u8; 32],
    pub new_protocol_version: u32,
    pub created_epoch: EpochId,
    pub voting_start_epoch: EpochId,
    pub voting_end_epoch: EpochId,
    pub approval_epoch: EpochId,
    pub activation_epoch: EpochId,
    pub leaf_hash: [u8; 32],
}

impl GovernanceProposalLeaf {
    pub fn from_hash_input(input: GovernanceProposalLeafHashInput) -> Self {
        let leaf_hash = input.leaf_hash();
        Self {
            version: input.version,
            proposal_id: input.proposal_id,
            source_domain: input.source_domain,
            target_domain: input.target_domain,
            target_domains: input.target_domains,
            proposer: input.proposer,
            payload_hash: input.payload_hash,
            new_protocol_version: input.new_protocol_version,
            created_epoch: input.created_epoch,
            voting_start_epoch: input.voting_start_epoch,
            voting_end_epoch: input.voting_end_epoch,
            approval_epoch: input.approval_epoch,
            activation_epoch: input.activation_epoch,
            leaf_hash,
        }
    }

    pub fn hash_input(&self) -> GovernanceProposalLeafHashInput {
        GovernanceProposalLeafHashInput {
            version: self.version,
            proposal_id: self.proposal_id,
            source_domain: self.source_domain,
            target_domain: self.target_domain,
            target_domains: self.target_domains.clone(),
            proposer: self.proposer,
            payload_hash: self.payload_hash,
            new_protocol_version: self.new_protocol_version,
            created_epoch: self.created_epoch,
            voting_start_epoch: self.voting_start_epoch,
            voting_end_epoch: self.voting_end_epoch,
            approval_epoch: self.approval_epoch,
            activation_epoch: self.activation_epoch,
        }
    }

    pub fn compute_leaf_hash(&self) -> [u8; 32] {
        self.hash_input().leaf_hash()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GovernanceAckLeafHashInput {
    pub version: u16,
    pub proposal_id: GovernanceProposalId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub acknowledging_domain: DomainId,
    pub target_domains: Vec<DomainId>,
    pub payload_hash: [u8; 32],
    pub new_protocol_version: u32,
    pub activation_epoch: EpochId,
    pub acknowledged_epoch: EpochId,
}

impl GovernanceAckLeafHashInput {
    pub fn leaf_hash(&self) -> [u8; 32] {
        blake2_256(&self.encode())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GovernanceAckLeaf {
    pub version: u16,
    pub proposal_id: GovernanceProposalId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub acknowledging_domain: DomainId,
    pub target_domains: Vec<DomainId>,
    pub payload_hash: [u8; 32],
    pub new_protocol_version: u32,
    pub activation_epoch: EpochId,
    pub acknowledged_epoch: EpochId,
    pub leaf_hash: [u8; 32],
}

impl GovernanceAckLeaf {
    pub fn from_hash_input(input: GovernanceAckLeafHashInput) -> Self {
        let leaf_hash = input.leaf_hash();
        Self {
            version: input.version,
            proposal_id: input.proposal_id,
            source_domain: input.source_domain,
            target_domain: input.target_domain,
            acknowledging_domain: input.acknowledging_domain,
            target_domains: input.target_domains,
            payload_hash: input.payload_hash,
            new_protocol_version: input.new_protocol_version,
            activation_epoch: input.activation_epoch,
            acknowledged_epoch: input.acknowledged_epoch,
            leaf_hash,
        }
    }

    pub fn hash_input(&self) -> GovernanceAckLeafHashInput {
        GovernanceAckLeafHashInput {
            version: self.version,
            proposal_id: self.proposal_id,
            source_domain: self.source_domain,
            target_domain: self.target_domain,
            acknowledging_domain: self.acknowledging_domain,
            target_domains: self.target_domains.clone(),
            payload_hash: self.payload_hash,
            new_protocol_version: self.new_protocol_version,
            activation_epoch: self.activation_epoch,
            acknowledged_epoch: self.acknowledged_epoch,
        }
    }

    pub fn compute_leaf_hash(&self) -> [u8; 32] {
        self.hash_input().leaf_hash()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum GovernanceLeaf {
    ProposalV1(GovernanceProposalLeaf),
    AckV1(GovernanceAckLeaf),
}

impl Default for GovernanceLeaf {
    fn default() -> Self {
        Self::ProposalV1(GovernanceProposalLeaf::default())
    }
}

impl GovernanceLeaf {
    pub fn target_domain(&self) -> DomainId {
        match self {
            Self::ProposalV1(leaf) => leaf.target_domain,
            Self::AckV1(leaf) => leaf.target_domain,
        }
    }

    pub fn proposal_id(&self) -> GovernanceProposalId {
        match self {
            Self::ProposalV1(leaf) => leaf.proposal_id,
            Self::AckV1(leaf) => leaf.proposal_id,
        }
    }

    pub fn leaf_hash(&self) -> [u8; 32] {
        match self {
            Self::ProposalV1(leaf) => leaf.leaf_hash,
            Self::AckV1(leaf) => leaf.leaf_hash,
        }
    }

    pub fn compute_leaf_hash(&self) -> [u8; 32] {
        match self {
            Self::ProposalV1(leaf) => leaf.compute_leaf_hash(),
            Self::AckV1(leaf) => leaf.compute_leaf_hash(),
        }
    }

    fn ordering_domain(&self) -> DomainId {
        match self {
            Self::ProposalV1(leaf) => leaf.source_domain,
            Self::AckV1(leaf) => leaf.acknowledging_domain,
        }
    }

    fn kind_order(&self) -> u8 {
        match self {
            Self::ProposalV1(_) => 0,
            Self::AckV1(_) => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GovernanceInclusionProof {
    pub version: u16,
    pub leaf: GovernanceLeaf,
    pub leaf_index: u32,
    pub leaf_count: u32,
    pub siblings: Vec<[u8; 32]>,
}

pub fn summary_header_storage_key(epoch_id: EpochId) -> Vec<u8> {
    storage_map_key(b"Epochs", b"SummaryHeaders", &epoch_id.encode())
}

pub fn epoch_export_ids_storage_key(epoch_id: EpochId) -> Vec<u8> {
    storage_map_key(b"Transfers", b"EpochExportIds", &epoch_id.encode())
}

pub fn export_record_storage_key(export_id: ExportId) -> Vec<u8> {
    storage_map_key(b"Transfers", b"ExportsById", &export_id.encode())
}

pub fn observed_import_storage_key(export_id: ExportId) -> Vec<u8> {
    storage_map_key(b"Transfers", b"ObservedImportsById", &export_id.encode())
}

pub fn epoch_finalized_import_ids_storage_key(epoch_id: EpochId) -> Vec<u8> {
    storage_map_key(b"Transfers", b"EpochFinalizedImportIds", &epoch_id.encode())
}

pub fn importer_account_storage_key() -> Vec<u8> {
    storage_value_key(b"Transfers", b"ImporterAccount")
}

pub fn governance_protocol_version_storage_key() -> Vec<u8> {
    storage_value_key(b"Governance", b"ProtocolVersion")
}

pub fn governance_importer_account_storage_key() -> Vec<u8> {
    storage_value_key(b"Governance", b"ImporterAccount")
}

pub fn governance_voters_storage_key() -> Vec<u8> {
    storage_value_key(b"Governance", b"GovernanceVoters")
}

pub fn governance_proposal_storage_key(proposal_id: GovernanceProposalId) -> Vec<u8> {
    storage_map_key(b"Governance", b"ProposalsById", &proposal_id.encode())
}

pub fn governance_activation_record_storage_key(proposal_id: GovernanceProposalId) -> Vec<u8> {
    storage_map_key(b"Governance", b"ActivationRecordsById", &proposal_id.encode())
}

pub fn governance_ack_record_storage_key(
    proposal_id: GovernanceProposalId,
    acknowledging_domain: DomainId,
) -> Vec<u8> {
    storage_map_key(
        b"Governance",
        b"AckRecordsByKey",
        &(proposal_id, acknowledging_domain).encode(),
    )
}

pub fn epoch_governance_leaf_ids_storage_key(epoch_id: EpochId) -> Vec<u8> {
    storage_map_key(b"Governance", b"EpochGovernanceLeafIds", &epoch_id.encode())
}

pub fn governance_leaf_storage_key(leaf_hash: [u8; 32]) -> Vec<u8> {
    storage_map_key(b"Governance", b"GovernanceLeavesById", &leaf_hash.encode())
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Default, Encode, Decode, DecodeWithMemTracking, TypeInfo,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CertificationPendingReason {
    #[default]
    TargetBlockNotFinalized,
    NoJustifiedDescendantYet,
    MissingTargetBlockHash,
    HistoricalStateUnavailable,
    StorageProofConstructionFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RelayPackageEnvelopeV1 {
    pub version: u16,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub epoch_id: EpochId,
    pub summary_hash: [u8; 32],
    pub package_hash: [u8; 32],
    pub package_bytes: Vec<u8>,
    pub export_count: u32,
    pub relay_submitted_at_unix_ms: u64,
}

impl RelayPackageEnvelopeV1 {
    pub fn new(
        source_domain: DomainId,
        target_domain: DomainId,
        epoch_id: EpochId,
        summary_hash: [u8; 32],
        package_hash: [u8; 32],
        package_bytes: Vec<u8>,
        export_count: u32,
        relay_submitted_at_unix_ms: u64,
    ) -> Self {
        Self {
            version: RELAY_PACKAGE_ENVELOPE_VERSION,
            source_domain,
            target_domain,
            epoch_id,
            summary_hash,
            package_hash,
            package_bytes,
            export_count,
            relay_submitted_at_unix_ms,
        }
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ImporterPackageState {
    #[default]
    Received,
    Verifying,
    SubmissionRetrying,
    AckedVerified,
    AckedDuplicateLocal,
    AckedDuplicateRemote,
    AckedInvalid,
}

impl ImporterPackageState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::AckedVerified
                | Self::AckedDuplicateLocal
                | Self::AckedDuplicateRemote
                | Self::AckedInvalid
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ImporterPackageStatusView {
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub epoch_id: EpochId,
    pub package_hash: [u8; 32],
    pub summary_hash: [u8; 32],
    pub state: ImporterPackageState,
    pub terminal: bool,
    pub reason: Option<String>,
    pub export_count: u32,
    pub tx_hashes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GrandpaFinalityCertificate {
    pub version: u16,
    pub grandpa_set_id: u64,
    pub target_block_number: u32,
    pub target_block_hash: [u8; 32],
    pub proof_block_number: u32,
    pub proof_block_hash: [u8; 32],
    pub justification: Vec<u8>,
    pub ancestry_headers: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SummaryHeaderStorageProof {
    pub version: u16,
    pub proof_block_number: u32,
    pub proof_block_hash: [u8; 32],
    /// SCALE-encoded canonical block header bytes for the proof block.
    pub proof_block_header: Vec<u8>,
    pub storage_key: Vec<u8>,
    pub trie_nodes: Vec<Vec<u8>>,
}

impl SummaryHeaderStorageProof {
    pub fn node_count(&self) -> usize {
        self.trie_nodes.len()
    }

    pub fn total_proof_bytes(&self) -> usize {
        self.trie_nodes.iter().map(Vec::len).sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ExportInclusionProof {
    pub version: u16,
    pub leaf: ExportLeaf,
    pub leaf_index: u32,
    pub leaf_count: u32,
    pub siblings: Vec<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct FinalizedImportLeafHashInput {
    pub version: u16,
    pub export_id: ExportId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub recipient: AccountIdBytes,
    pub amount: u128,
    pub source_epoch_id: EpochId,
    pub summary_hash: [u8; 32],
    pub package_hash: [u8; 32],
}

impl FinalizedImportLeafHashInput {
    pub fn import_hash(&self) -> [u8; 32] {
        blake2_256(&self.encode())
    }
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct FinalizedImportLeaf {
    pub version: u16,
    pub export_id: ExportId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub recipient: AccountIdBytes,
    pub amount: u128,
    pub source_epoch_id: EpochId,
    pub summary_hash: [u8; 32],
    pub package_hash: [u8; 32],
    pub import_hash: [u8; 32],
}

impl FinalizedImportLeaf {
    pub fn from_hash_input(input: FinalizedImportLeafHashInput) -> Self {
        Self {
            version: input.version,
            export_id: input.export_id,
            source_domain: input.source_domain,
            target_domain: input.target_domain,
            recipient: input.recipient,
            amount: input.amount,
            source_epoch_id: input.source_epoch_id,
            summary_hash: input.summary_hash,
            package_hash: input.package_hash,
            import_hash: input.import_hash(),
        }
    }

    pub fn hash_input(&self) -> FinalizedImportLeafHashInput {
        FinalizedImportLeafHashInput {
            version: self.version,
            export_id: self.export_id,
            source_domain: self.source_domain,
            target_domain: self.target_domain,
            recipient: self.recipient,
            amount: self.amount,
            source_epoch_id: self.source_epoch_id,
            summary_hash: self.summary_hash,
            package_hash: self.package_hash,
        }
    }

    pub fn compute_import_hash(&self) -> [u8; 32] {
        self.hash_input().import_hash()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct FinalizedImportInclusionProof {
    pub version: u16,
    pub leaf: FinalizedImportLeaf,
    pub leaf_index: u32,
    pub leaf_count: u32,
    pub siblings: Vec<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RemoteFinalizationClaim {
    pub version: u16,
    pub export_id: ExportId,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub source_epoch_id: EpochId,
    pub recipient: AccountIdBytes,
    pub amount: u128,
    pub completion_summary_hash: [u8; 32],
    pub completion_package_hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum InclusionProof {
    SummaryHeaderStorageV1(SummaryHeaderStorageProof),
    ExportV1(ExportInclusionProof),
    FinalizedImportV1(FinalizedImportInclusionProof),
    GovernanceV1(GovernanceInclusionProof),
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SummaryCertificationBundle {
    pub certificate: SummaryCertificate,
    pub summary_header_storage_proof: SummaryHeaderStorageProof,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum SummaryCertificate {
    GrandpaV1(GrandpaFinalityCertificate),
}

impl Default for SummaryCertificate {
    fn default() -> Self {
        Self::GrandpaV1(GrandpaFinalityCertificate::default())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum SummaryCertificationState {
    Pending(CertificationPendingReason),
    Ready(SummaryCertificationBundle),
}

impl Default for SummaryCertificationState {
    fn default() -> Self {
        Self::Pending(CertificationPendingReason::default())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SummaryCertificationReadiness {
    pub epoch_id: EpochId,
    pub staged_at_block_number: u32,
    pub staged_at_block_hash: [u8; 32],
    pub latest_finalized_block_number: u32,
    pub latest_finalized_block_hash: [u8; 32],
    pub state: SummaryCertificationState,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CertifiedSummaryPackageHashInput {
    pub version: u16,
    pub header: EpochSummaryHeader,
    pub certificate: SummaryCertificate,
    pub inclusion_proofs: Vec<Vec<u8>>,
    pub artifacts: Vec<Vec<u8>>,
}

impl CertifiedSummaryPackageHashInput {
    pub fn package_hash(&self) -> [u8; 32] {
        blake2_256(&self.encode())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CertifiedSummaryPackage {
    pub version: u16,
    pub package_hash: [u8; 32],
    pub header: EpochSummaryHeader,
    pub certificate: SummaryCertificate,
    pub inclusion_proofs: Vec<Vec<u8>>,
    pub artifacts: Vec<Vec<u8>>,
}

impl CertifiedSummaryPackage {
    pub fn from_fields(
        header: EpochSummaryHeader,
        certificate: SummaryCertificate,
        inclusion_proofs: Vec<Vec<u8>>,
        artifacts: Vec<Vec<u8>>,
    ) -> Self {
        let package = Self {
            version: CERTIFIED_SUMMARY_PACKAGE_VERSION,
            package_hash: EMPTY_HASH,
            header,
            certificate,
            inclusion_proofs,
            artifacts,
        };

        Self {
            package_hash: package.compute_package_hash(),
            ..package
        }
    }

    pub fn from_parts(header: EpochSummaryHeader, certificate: SummaryCertificate) -> Self {
        Self::from_fields(header, certificate, Vec::new(), Vec::new())
    }

    pub fn from_bundle(header: EpochSummaryHeader, bundle: SummaryCertificationBundle) -> Self {
        Self::from_bundle_with_export_proofs(header, bundle, Vec::new())
    }

    pub fn from_bundle_with_export_proofs(
        header: EpochSummaryHeader,
        bundle: SummaryCertificationBundle,
        export_proofs: Vec<ExportInclusionProof>,
    ) -> Self {
        Self::from_bundle_with_mixed_proofs(header, bundle, export_proofs, Vec::new(), Vec::new())
    }

    pub fn from_bundle_with_finalized_import_proofs(
        header: EpochSummaryHeader,
        bundle: SummaryCertificationBundle,
        finalized_import_proofs: Vec<FinalizedImportInclusionProof>,
    ) -> Self {
        Self::from_bundle_with_mixed_proofs(
            header,
            bundle,
            Vec::new(),
            finalized_import_proofs,
            Vec::new(),
        )
    }

    pub fn from_bundle_with_governance_proofs(
        header: EpochSummaryHeader,
        bundle: SummaryCertificationBundle,
        governance_proofs: Vec<GovernanceInclusionProof>,
    ) -> Self {
        Self::from_bundle_with_mixed_proofs(
            header,
            bundle,
            Vec::new(),
            Vec::new(),
            governance_proofs,
        )
    }

    pub fn from_bundle_with_mixed_proofs(
        header: EpochSummaryHeader,
        bundle: SummaryCertificationBundle,
        export_proofs: Vec<ExportInclusionProof>,
        finalized_import_proofs: Vec<FinalizedImportInclusionProof>,
        governance_proofs: Vec<GovernanceInclusionProof>,
    ) -> Self {
        let mut inclusion_proofs =
            vec![
                InclusionProof::SummaryHeaderStorageV1(bundle.summary_header_storage_proof)
                    .encode(),
            ];
        inclusion_proofs.extend(
            export_proofs
                .into_iter()
                .map(InclusionProof::ExportV1)
                .map(|proof| proof.encode()),
        );
        inclusion_proofs.extend(
            finalized_import_proofs
                .into_iter()
                .map(InclusionProof::FinalizedImportV1)
                .map(|proof| proof.encode()),
        );
        inclusion_proofs.extend(
            governance_proofs
                .into_iter()
                .map(InclusionProof::GovernanceV1)
                .map(|proof| proof.encode()),
        );
        Self::from_fields(header, bundle.certificate, inclusion_proofs, Vec::new())
    }

    pub fn hash_input(&self) -> CertifiedSummaryPackageHashInput {
        CertifiedSummaryPackageHashInput {
            version: self.version,
            header: self.header.clone(),
            certificate: self.certificate.clone(),
            inclusion_proofs: self.inclusion_proofs.clone(),
            artifacts: self.artifacts.clone(),
        }
    }

    pub fn compute_package_hash(&self) -> [u8; 32] {
        self.hash_input().package_hash()
    }
}

pub fn fixed_bytes<const N: usize>(value: &[u8]) -> [u8; N] {
    let mut output = [0u8; N];
    output[..value.len()].copy_from_slice(value);
    output
}

pub fn export_id(source_domain: DomainId, export_sequence: u64) -> ExportId {
    blake2_256(&(source_domain, export_sequence).encode())
}

pub fn governance_proposal_id(
    source_domain: DomainId,
    proposal_sequence: u64,
) -> GovernanceProposalId {
    blake2_256(&(source_domain, proposal_sequence).encode())
}

pub fn export_leaf_ordering(left: &ExportLeaf, right: &ExportLeaf) -> Ordering {
    left.source_block_height
        .cmp(&right.source_block_height)
        .then(left.extrinsic_index.cmp(&right.extrinsic_index))
        .then(left.export_id.cmp(&right.export_id))
}

pub fn sort_export_leaves(leaves: &mut [ExportLeaf]) {
    leaves.sort_by(export_leaf_ordering);
}

pub fn export_merkle_empty_root(
    domain_id: DomainId,
    epoch_id: EpochId,
    start_block_height: u32,
    end_block_height: u32,
) -> [u8; 32] {
    blake2_256(
        &(
            EXPORT_MERKLE_EMPTY_LABEL,
            domain_id,
            epoch_id,
            start_block_height,
            end_block_height,
        )
            .encode(),
    )
}

pub fn export_merkle_root(
    domain_id: DomainId,
    epoch_id: EpochId,
    start_block_height: u32,
    end_block_height: u32,
    leaves: &[ExportLeaf],
) -> [u8; 32] {
    let mut ordered = leaves.to_vec();
    sort_export_leaves(&mut ordered);

    if ordered.is_empty() {
        return export_merkle_empty_root(domain_id, epoch_id, start_block_height, end_block_height);
    }

    let mut level = ordered
        .iter()
        .map(|leaf| leaf.export_hash)
        .collect::<Vec<_>>();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut index = 0usize;
        while index < level.len() {
            let left = level[index];
            let right = if index + 1 < level.len() {
                level[index + 1]
            } else {
                left
            };
            next.push(export_merkle_parent_hash(left, right));
            index += 2;
        }
        level = next;
    }

    level[0]
}

pub fn build_export_inclusion_proof(
    leaves: &[ExportLeaf],
    export_id: ExportId,
) -> Option<ExportInclusionProof> {
    let mut ordered = leaves.to_vec();
    sort_export_leaves(&mut ordered);

    let leaf_index = ordered
        .iter()
        .position(|leaf| leaf.export_id == export_id)?;
    let leaf_count = ordered.len();
    let mut siblings = Vec::new();
    let mut index = leaf_index;
    let mut level = ordered
        .iter()
        .map(|leaf| leaf.export_hash)
        .collect::<Vec<_>>();

    while level.len() > 1 {
        let sibling_index = if index % 2 == 0 {
            if index + 1 < level.len() {
                index + 1
            } else {
                index
            }
        } else {
            index - 1
        };
        siblings.push(level[sibling_index]);

        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut pair_index = 0usize;
        while pair_index < level.len() {
            let left = level[pair_index];
            let right = if pair_index + 1 < level.len() {
                level[pair_index + 1]
            } else {
                left
            };
            next.push(export_merkle_parent_hash(left, right));
            pair_index += 2;
        }
        index /= 2;
        level = next;
    }

    Some(ExportInclusionProof {
        version: EXPORT_INCLUSION_PROOF_VERSION,
        leaf: ordered[leaf_index].clone(),
        leaf_index: leaf_index as u32,
        leaf_count: leaf_count as u32,
        siblings,
    })
}

pub fn verify_export_inclusion_proof(export_root: [u8; 32], proof: &ExportInclusionProof) -> bool {
    if proof.leaf.export_hash != proof.leaf.compute_export_hash() {
        return false;
    }
    if proof.leaf_count == 0 || proof.leaf_index >= proof.leaf_count {
        return false;
    }

    let mut index = proof.leaf_index as usize;
    let mut width = proof.leaf_count as usize;
    let mut hash = proof.leaf.export_hash;

    if width == 1 {
        return hash == export_root && proof.siblings.is_empty();
    }

    for sibling in &proof.siblings {
        let sibling_hash = *sibling;
        let is_right_duplicate = width % 2 == 1 && index == width - 1;
        let (left, right) = if index.is_multiple_of(2) {
            (
                hash,
                if is_right_duplicate {
                    hash
                } else {
                    sibling_hash
                },
            )
        } else {
            (sibling_hash, hash)
        };
        hash = export_merkle_parent_hash(left, right);
        index /= 2;
        width = width.div_ceil(2);
    }

    hash == export_root
}

pub fn seed_epoch_accumulator(label: &str, domain_id: DomainId, epoch_id: EpochId) -> [u8; 32] {
    blake2_256(&(label, EPOCH_SUMMARY_VERSION, domain_id, epoch_id).encode())
}

pub fn fold_epoch_accumulator(
    accumulator: [u8; 32],
    block_number: u32,
    payload_hash: [u8; 32],
) -> [u8; 32] {
    blake2_256(&(accumulator, block_number, payload_hash).encode())
}

pub fn tx_envelope_hash(
    block_number: u32,
    extrinsic_count: u32,
    all_extrinsics_len: u32,
) -> [u8; 32] {
    blake2_256(&(block_number, extrinsic_count, all_extrinsics_len).encode())
}

pub fn event_envelope_hash(block_number: u32, event_count: u32) -> [u8; 32] {
    blake2_256(&(block_number, event_count).encode())
}

pub fn empty_commitment_root(
    label: &str,
    domain_id: DomainId,
    epoch_id: EpochId,
    start_block_height: u32,
    end_block_height: u32,
) -> [u8; 32] {
    blake2_256(
        &(
            label,
            EPOCH_SUMMARY_VERSION,
            domain_id,
            epoch_id,
            start_block_height,
            end_block_height,
        )
            .encode(),
    )
}

pub fn validator_set_hash<Authorities: Encode>(
    grandpa_set_id: u64,
    ordered_grandpa_authorities: &Authorities,
) -> [u8; 32] {
    blake2_256(&(grandpa_set_id, ordered_grandpa_authorities).encode())
}

pub fn export_merkle_parent_hash(left: [u8; 32], right: [u8; 32]) -> [u8; 32] {
    blake2_256(&(EXPORT_MERKLE_NODE_LABEL, left, right).encode())
}

pub fn finalized_import_leaf_ordering(
    left: &FinalizedImportLeaf,
    right: &FinalizedImportLeaf,
) -> Ordering {
    left.export_id.cmp(&right.export_id)
}

pub fn sort_finalized_import_leaves(leaves: &mut [FinalizedImportLeaf]) {
    leaves.sort_by(finalized_import_leaf_ordering);
}

pub fn import_merkle_empty_root(
    domain_id: DomainId,
    epoch_id: EpochId,
    start_block_height: u32,
    end_block_height: u32,
) -> [u8; 32] {
    blake2_256(
        &(
            IMPORT_MERKLE_EMPTY_LABEL,
            domain_id,
            epoch_id,
            start_block_height,
            end_block_height,
        )
            .encode(),
    )
}

pub fn import_merkle_parent_hash(left: [u8; 32], right: [u8; 32]) -> [u8; 32] {
    blake2_256(&(IMPORT_MERKLE_NODE_LABEL, left, right).encode())
}

pub fn import_merkle_root(
    domain_id: DomainId,
    epoch_id: EpochId,
    start_block_height: u32,
    end_block_height: u32,
    leaves: &[FinalizedImportLeaf],
) -> [u8; 32] {
    let mut ordered = leaves.to_vec();
    sort_finalized_import_leaves(&mut ordered);

    if ordered.is_empty() {
        return import_merkle_empty_root(domain_id, epoch_id, start_block_height, end_block_height);
    }

    let mut level = ordered
        .iter()
        .map(|leaf| leaf.import_hash)
        .collect::<Vec<_>>();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut index = 0usize;
        while index < level.len() {
            let left = level[index];
            let right = if index + 1 < level.len() {
                level[index + 1]
            } else {
                left
            };
            next.push(import_merkle_parent_hash(left, right));
            index += 2;
        }
        level = next;
    }

    level[0]
}

pub fn build_finalized_import_inclusion_proof(
    leaves: &[FinalizedImportLeaf],
    export_id: ExportId,
) -> Option<FinalizedImportInclusionProof> {
    let mut ordered = leaves.to_vec();
    sort_finalized_import_leaves(&mut ordered);

    let leaf_index = ordered.iter().position(|leaf| leaf.export_id == export_id)?;
    let leaf_count = ordered.len();
    let mut siblings = Vec::new();
    let mut index = leaf_index;
    let mut level = ordered
        .iter()
        .map(|leaf| leaf.import_hash)
        .collect::<Vec<_>>();

    while level.len() > 1 {
        let sibling_index = if index % 2 == 0 {
            if index + 1 < level.len() {
                index + 1
            } else {
                index
            }
        } else {
            index - 1
        };
        siblings.push(level[sibling_index]);

        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut pair_index = 0usize;
        while pair_index < level.len() {
            let left = level[pair_index];
            let right = if pair_index + 1 < level.len() {
                level[pair_index + 1]
            } else {
                left
            };
            next.push(import_merkle_parent_hash(left, right));
            pair_index += 2;
        }
        index /= 2;
        level = next;
    }

    Some(FinalizedImportInclusionProof {
        version: FINALIZED_IMPORT_INCLUSION_PROOF_VERSION,
        leaf: ordered[leaf_index].clone(),
        leaf_index: leaf_index as u32,
        leaf_count: leaf_count as u32,
        siblings,
    })
}

pub fn verify_finalized_import_inclusion_proof(
    import_root: [u8; 32],
    proof: &FinalizedImportInclusionProof,
) -> bool {
    if proof.leaf.import_hash != proof.leaf.compute_import_hash() {
        return false;
    }
    if proof.leaf_count == 0 || proof.leaf_index >= proof.leaf_count {
        return false;
    }

    let mut index = proof.leaf_index as usize;
    let mut width = proof.leaf_count as usize;
    let mut hash = proof.leaf.import_hash;

    if width == 1 {
        return hash == import_root && proof.siblings.is_empty();
    }

    for sibling in &proof.siblings {
        let sibling_hash = *sibling;
        let is_right_duplicate = width % 2 == 1 && index == width - 1;
        let (left, right) = if index.is_multiple_of(2) {
            (
                hash,
                if is_right_duplicate {
                    hash
                } else {
                    sibling_hash
                },
            )
        } else {
            (sibling_hash, hash)
        };
        hash = import_merkle_parent_hash(left, right);
        index /= 2;
        width = width.div_ceil(2);
    }

    hash == import_root
}

pub fn governance_leaf_ordering(left: &GovernanceLeaf, right: &GovernanceLeaf) -> Ordering {
    left.target_domain()
        .cmp(&right.target_domain())
        .then(left.kind_order().cmp(&right.kind_order()))
        .then(left.proposal_id().cmp(&right.proposal_id()))
        .then(left.ordering_domain().cmp(&right.ordering_domain()))
}

pub fn sort_governance_leaves(leaves: &mut [GovernanceLeaf]) {
    leaves.sort_by(governance_leaf_ordering);
}

pub fn governance_merkle_empty_root(
    domain_id: DomainId,
    epoch_id: EpochId,
    start_block_height: u32,
    end_block_height: u32,
) -> [u8; 32] {
    blake2_256(
        &(
            GOVERNANCE_MERKLE_EMPTY_LABEL,
            domain_id,
            epoch_id,
            start_block_height,
            end_block_height,
        )
            .encode(),
    )
}

pub fn governance_merkle_parent_hash(left: [u8; 32], right: [u8; 32]) -> [u8; 32] {
    blake2_256(&(GOVERNANCE_MERKLE_NODE_LABEL, left, right).encode())
}

pub fn governance_merkle_root(
    domain_id: DomainId,
    epoch_id: EpochId,
    start_block_height: u32,
    end_block_height: u32,
    leaves: &[GovernanceLeaf],
) -> [u8; 32] {
    let mut ordered = leaves.to_vec();
    sort_governance_leaves(&mut ordered);

    if ordered.is_empty() {
        return governance_merkle_empty_root(
            domain_id,
            epoch_id,
            start_block_height,
            end_block_height,
        );
    }

    let mut level = ordered.iter().map(GovernanceLeaf::leaf_hash).collect::<Vec<_>>();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut index = 0usize;
        while index < level.len() {
            let left = level[index];
            let right = if index + 1 < level.len() {
                level[index + 1]
            } else {
                left
            };
            next.push(governance_merkle_parent_hash(left, right));
            index += 2;
        }
        level = next;
    }

    level[0]
}

pub fn build_governance_inclusion_proof(
    leaves: &[GovernanceLeaf],
    target_leaf_hash: [u8; 32],
) -> Option<GovernanceInclusionProof> {
    let mut ordered = leaves.to_vec();
    sort_governance_leaves(&mut ordered);

    let leaf_index = ordered
        .iter()
        .position(|leaf| leaf.leaf_hash() == target_leaf_hash)?;
    let leaf_count = ordered.len();
    let mut siblings = Vec::new();
    let mut index = leaf_index;
    let mut level = ordered.iter().map(GovernanceLeaf::leaf_hash).collect::<Vec<_>>();

    while level.len() > 1 {
        let sibling_index = if index % 2 == 0 {
            if index + 1 < level.len() {
                index + 1
            } else {
                index
            }
        } else {
            index - 1
        };
        siblings.push(level[sibling_index]);

        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut pair_index = 0usize;
        while pair_index < level.len() {
            let left = level[pair_index];
            let right = if pair_index + 1 < level.len() {
                level[pair_index + 1]
            } else {
                left
            };
            next.push(governance_merkle_parent_hash(left, right));
            pair_index += 2;
        }
        index /= 2;
        level = next;
    }

    Some(GovernanceInclusionProof {
        version: GOVERNANCE_INCLUSION_PROOF_VERSION,
        leaf: ordered[leaf_index].clone(),
        leaf_index: leaf_index as u32,
        leaf_count: leaf_count as u32,
        siblings,
    })
}

pub fn verify_governance_inclusion_proof(
    governance_root: [u8; 32],
    proof: &GovernanceInclusionProof,
) -> bool {
    if proof.leaf.leaf_hash() != proof.leaf.compute_leaf_hash() {
        return false;
    }
    if proof.leaf_count == 0 || proof.leaf_index >= proof.leaf_count {
        return false;
    }

    let mut index = proof.leaf_index as usize;
    let mut width = proof.leaf_count as usize;
    let mut hash = proof.leaf.leaf_hash();

    if width == 1 {
        return hash == governance_root && proof.siblings.is_empty();
    }

    for sibling in &proof.siblings {
        let sibling_hash = *sibling;
        let is_right_duplicate = width % 2 == 1 && index == width - 1;
        let (left, right) = if index.is_multiple_of(2) {
            (
                hash,
                if is_right_duplicate {
                    hash
                } else {
                    sibling_hash
                },
            )
        } else {
            (sibling_hash, hash)
        };
        hash = governance_merkle_parent_hash(left, right);
        index /= 2;
        width = width.div_ceil(2);
    }

    hash == governance_root
}

pub fn storage_value_key(pallet: &[u8], storage: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(32);
    key.extend_from_slice(&twox_128(pallet));
    key.extend_from_slice(&twox_128(storage));
    key
}

pub fn storage_map_key(pallet: &[u8], storage: &[u8], encoded_key: &[u8]) -> Vec<u8> {
    let mut key = storage_value_key(pallet, storage);
    key.extend_from_slice(&blake2_128(encoded_key));
    key.extend_from_slice(encoded_key);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hash_input() -> EpochSummaryHashInput {
        EpochSummaryHashInput {
            version: EPOCH_SUMMARY_VERSION,
            domain_id: DomainId::Earth,
            epoch_id: 7,
            prev_summary_hash: [1u8; 32],
            start_block_height: 41,
            end_block_height: 60,
            state_root: [2u8; 32],
            block_root: [3u8; 32],
            tx_root: [4u8; 32],
            event_root: [5u8; 32],
            export_root: [6u8; 32],
            import_root: [7u8; 32],
            governance_root: [8u8; 32],
            validator_set_hash: [9u8; 32],
        }
    }

    #[test]
    fn summary_hash_is_deterministic_for_identical_input() {
        let first = sample_hash_input();
        let second = sample_hash_input();

        assert_eq!(first.summary_hash(), second.summary_hash());
    }

    #[test]
    fn changing_hashed_field_changes_summary_hash() {
        let first = sample_hash_input();
        let mut second = sample_hash_input();
        second.event_root = [11u8; 32];

        assert_ne!(first.summary_hash(), second.summary_hash());
    }

    #[test]
    fn placeholder_roots_are_deterministic_and_label_distinct() {
        let export = empty_commitment_root(EXPORT_ROOT_EMPTY_LABEL, DomainId::Moon, 3, 10, 12);
        let export_again =
            empty_commitment_root(EXPORT_ROOT_EMPTY_LABEL, DomainId::Moon, 3, 10, 12);
        let import = empty_commitment_root(IMPORT_ROOT_EMPTY_LABEL, DomainId::Moon, 3, 10, 12);

        assert_eq!(export, export_again);
        assert_ne!(export, import);
    }

    #[test]
    fn validator_set_hash_is_stable_for_identical_authority_input() {
        let authorities = vec![([1u8; 32], 1u64), ([2u8; 32], 1u64)];

        let first = validator_set_hash(9, &authorities);
        let second = validator_set_hash(9, &authorities);

        assert_eq!(first, second);
    }

    #[test]
    fn header_round_trips_through_hash_input() {
        let input = sample_hash_input();
        let header = EpochSummaryHeader::from_hash_input(input.clone());

        assert_eq!(header.hash_input(), input);
        assert_eq!(header.summary_hash, header.compute_summary_hash());
    }

    fn sample_certificate() -> SummaryCertificate {
        SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
            version: GRANDPA_FINALITY_CERTIFICATE_VERSION,
            grandpa_set_id: 1,
            target_block_number: 60,
            target_block_hash: [10u8; 32],
            proof_block_number: 63,
            proof_block_hash: [11u8; 32],
            justification: vec![1, 2, 3],
            ancestry_headers: vec![vec![4, 5, 6]],
        })
    }

    #[test]
    fn package_hash_is_deterministic_for_identical_input() {
        let header = EpochSummaryHeader::from_hash_input(sample_hash_input());
        let first = CertifiedSummaryPackage::from_parts(header.clone(), sample_certificate());
        let second = CertifiedSummaryPackage::from_parts(header, sample_certificate());

        assert_eq!(first.package_hash, second.package_hash);
        assert!(first.inclusion_proofs.is_empty());
        assert!(first.artifacts.is_empty());
    }

    fn sample_storage_proof() -> SummaryHeaderStorageProof {
        SummaryHeaderStorageProof {
            version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
            proof_block_number: 63,
            proof_block_hash: [12u8; 32],
            proof_block_header: vec![7, 8, 9],
            storage_key: summary_header_storage_key(7),
            trie_nodes: vec![vec![1, 2], vec![3, 4, 5]],
        }
    }

    fn sample_export_leaf(
        export_id: ExportId,
        target_domain: DomainId,
        source_block_height: u32,
        extrinsic_index: u32,
    ) -> ExportLeaf {
        ExportLeaf::from_hash_input(ExportLeafHashInput {
            version: EXPORT_LEAF_VERSION,
            export_id,
            source_domain: DomainId::Earth,
            target_domain,
            sender: [21u8; 32],
            recipient: [22u8; 32],
            amount: 99,
            source_epoch_id: 7,
            source_block_height,
            extrinsic_index,
        })
    }

    fn sample_governance_proposal_leaf(
        proposal_id: GovernanceProposalId,
        target_domain: DomainId,
    ) -> GovernanceLeaf {
        GovernanceLeaf::ProposalV1(GovernanceProposalLeaf::from_hash_input(
            GovernanceProposalLeafHashInput {
                version: GOVERNANCE_PROPOSAL_LEAF_VERSION,
                proposal_id,
                source_domain: DomainId::Earth,
                target_domain,
                target_domains: vec![DomainId::Moon],
                proposer: [44u8; 32],
                payload_hash: GovernancePayload::SetProtocolVersion { new_version: 2 }
                    .payload_hash(),
                new_protocol_version: 2,
                created_epoch: 7,
                voting_start_epoch: 7,
                voting_end_epoch: 8,
                approval_epoch: 8,
                activation_epoch: 12,
            },
        ))
    }

    fn sample_governance_ack_leaf(
        proposal_id: GovernanceProposalId,
        target_domain: DomainId,
        acknowledging_domain: DomainId,
        acknowledged_epoch: EpochId,
    ) -> GovernanceLeaf {
        GovernanceLeaf::AckV1(GovernanceAckLeaf::from_hash_input(
            GovernanceAckLeafHashInput {
                version: GOVERNANCE_ACK_LEAF_VERSION,
                proposal_id,
                source_domain: DomainId::Earth,
                target_domain,
                acknowledging_domain,
                target_domains: vec![DomainId::Moon],
                payload_hash: GovernancePayload::SetProtocolVersion { new_version: 2 }
                    .payload_hash(),
                new_protocol_version: 2,
                activation_epoch: 12,
                acknowledged_epoch,
            },
        ))
    }

    #[test]
    fn summary_header_storage_key_is_deterministic() {
        assert_eq!(summary_header_storage_key(7), summary_header_storage_key(7));
    }

    #[test]
    fn inclusion_proof_round_trips_as_summary_header_storage_v1() {
        let encoded = InclusionProof::SummaryHeaderStorageV1(sample_storage_proof()).encode();
        let decoded = InclusionProof::decode(&mut &encoded[..]).expect("proof should decode");

        assert_eq!(
            decoded,
            InclusionProof::SummaryHeaderStorageV1(sample_storage_proof())
        );
    }

    #[test]
    fn package_hash_is_deterministic_with_inclusion_proof() {
        let header = EpochSummaryHeader::from_hash_input(sample_hash_input());
        let bundle = SummaryCertificationBundle {
            certificate: sample_certificate(),
            summary_header_storage_proof: sample_storage_proof(),
        };
        let first = CertifiedSummaryPackage::from_bundle(header.clone(), bundle.clone());
        let second = CertifiedSummaryPackage::from_bundle(header, bundle);

        assert_eq!(first.package_hash, second.package_hash);
        assert_eq!(first.inclusion_proofs.len(), 1);
        assert!(first.artifacts.is_empty());
    }

    #[test]
    fn changing_inclusion_proof_changes_package_hash() {
        let header = EpochSummaryHeader::from_hash_input(sample_hash_input());
        let first = CertifiedSummaryPackage::from_bundle(
            header.clone(),
            SummaryCertificationBundle {
                certificate: sample_certificate(),
                summary_header_storage_proof: sample_storage_proof(),
            },
        );
        let mut changed_proof = sample_storage_proof();
        changed_proof.trie_nodes[0].push(9);
        let second = CertifiedSummaryPackage::from_bundle(
            header,
            SummaryCertificationBundle {
                certificate: sample_certificate(),
                summary_header_storage_proof: changed_proof,
            },
        );

        assert_ne!(first.package_hash, second.package_hash);
    }

    #[test]
    fn export_leaf_hash_is_deterministic() {
        let first = sample_export_leaf([31u8; 32], DomainId::Moon, 50, 0);
        let second = sample_export_leaf([31u8; 32], DomainId::Moon, 50, 0);

        assert_eq!(first.export_hash, second.export_hash);
        assert_eq!(first.export_hash, first.compute_export_hash());
    }

    #[test]
    fn export_merkle_root_is_deterministic_and_proof_verifies() {
        let leaves = vec![
            sample_export_leaf([40u8; 32], DomainId::Moon, 52, 1),
            sample_export_leaf([39u8; 32], DomainId::Moon, 51, 0),
            sample_export_leaf([41u8; 32], DomainId::Mars, 52, 2),
        ];
        let root = export_merkle_root(DomainId::Earth, 7, 41, 60, &leaves);
        let same_root = export_merkle_root(DomainId::Earth, 7, 41, 60, &leaves);
        let proof = build_export_inclusion_proof(&leaves, [39u8; 32]).expect("proof");

        assert_eq!(root, same_root);
        assert!(verify_export_inclusion_proof(root, &proof));
    }

    #[test]
    fn export_proof_round_trips_as_export_v1() {
        let proof = build_export_inclusion_proof(
            &[
                sample_export_leaf([50u8; 32], DomainId::Moon, 60, 0),
                sample_export_leaf([51u8; 32], DomainId::Moon, 60, 1),
            ],
            [50u8; 32],
        )
        .expect("proof");
        let encoded = InclusionProof::ExportV1(proof.clone()).encode();
        let decoded = InclusionProof::decode(&mut &encoded[..]).expect("proof should decode");

        assert_eq!(decoded, InclusionProof::ExportV1(proof));
    }

    #[test]
    fn changing_export_proof_changes_package_hash() {
        let header = EpochSummaryHeader::from_hash_input(sample_hash_input());
        let bundle = SummaryCertificationBundle {
            certificate: sample_certificate(),
            summary_header_storage_proof: sample_storage_proof(),
        };
        let leaves = vec![
            sample_export_leaf([61u8; 32], DomainId::Moon, 61, 0),
            sample_export_leaf([62u8; 32], DomainId::Moon, 61, 1),
        ];
        let first = CertifiedSummaryPackage::from_bundle_with_export_proofs(
            header.clone(),
            bundle.clone(),
            vec![build_export_inclusion_proof(&leaves, [61u8; 32]).expect("proof")],
        );
        let mut changed_proof = build_export_inclusion_proof(&leaves, [61u8; 32]).expect("proof");
        changed_proof.siblings[0][0] ^= 0xFF;
        let second = CertifiedSummaryPackage::from_bundle_with_export_proofs(
            header,
            bundle,
            vec![changed_proof],
        );

        assert_ne!(first.package_hash, second.package_hash);
    }

    #[test]
    fn changing_certificate_changes_package_hash() {
        let header = EpochSummaryHeader::from_hash_input(sample_hash_input());
        let first = CertifiedSummaryPackage::from_parts(header.clone(), sample_certificate());
        let mut changed_certificate = sample_certificate();
        let SummaryCertificate::GrandpaV1(ref mut certificate) = changed_certificate;
        certificate.justification.push(9);
        let second = CertifiedSummaryPackage::from_parts(header, changed_certificate);

        assert_ne!(first.package_hash, second.package_hash);
    }

    #[test]
    fn package_domain_and_epoch_linkage_match_header() {
        let header = EpochSummaryHeader::from_hash_input(sample_hash_input());
        let package = CertifiedSummaryPackage::from_parts(header.clone(), sample_certificate());

        assert_eq!(package.header.domain_id, DomainId::Earth);
        assert_eq!(package.header.epoch_id, 7);
        assert_eq!(package.compute_package_hash(), package.package_hash);
    }

    #[test]
    fn governance_root_is_deterministic_and_proof_verifies() {
        let leaves = vec![
            sample_governance_ack_leaf(
                governance_proposal_id(DomainId::Earth, 0),
                DomainId::Earth,
                DomainId::Moon,
                9,
            ),
            sample_governance_proposal_leaf(governance_proposal_id(DomainId::Earth, 0), DomainId::Moon),
        ];
        let root = governance_merkle_root(DomainId::Earth, 8, 41, 60, &leaves);
        let same_root = governance_merkle_root(DomainId::Earth, 8, 41, 60, &leaves);
        let proof = build_governance_inclusion_proof(
            &leaves,
            sample_governance_proposal_leaf(governance_proposal_id(DomainId::Earth, 0), DomainId::Moon)
                .leaf_hash(),
        )
        .expect("governance proof");

        assert_eq!(root, same_root);
        assert!(verify_governance_inclusion_proof(root, &proof));
    }

    #[test]
    fn mixed_family_package_order_remains_summary_export_import_governance() {
        let header = EpochSummaryHeader::from_hash_input(sample_hash_input());
        let bundle = SummaryCertificationBundle {
            certificate: sample_certificate(),
            summary_header_storage_proof: sample_storage_proof(),
        };
        let export_leaf = sample_export_leaf([71u8; 32], DomainId::Moon, 61, 0);
        let import_leaf = FinalizedImportLeaf::from_hash_input(FinalizedImportLeafHashInput {
            version: FINALIZED_IMPORT_LEAF_VERSION,
            export_id: [72u8; 32],
            source_domain: DomainId::Moon,
            target_domain: DomainId::Earth,
            recipient: [73u8; 32],
            amount: 11,
            source_epoch_id: 6,
            summary_hash: [74u8; 32],
            package_hash: [75u8; 32],
        });
        let governance_leaf = sample_governance_proposal_leaf(
            governance_proposal_id(DomainId::Earth, 1),
            DomainId::Moon,
        );
        let package = CertifiedSummaryPackage::from_bundle_with_mixed_proofs(
            header,
            bundle,
            vec![build_export_inclusion_proof(core::slice::from_ref(&export_leaf), export_leaf.export_id)
                .expect("export proof")],
            vec![build_finalized_import_inclusion_proof(
                core::slice::from_ref(&import_leaf),
                import_leaf.export_id,
            )
            .expect("import proof")],
            vec![build_governance_inclusion_proof(
                core::slice::from_ref(&governance_leaf),
                governance_leaf.leaf_hash(),
            )
            .expect("governance proof")],
        );

        assert!(matches!(
            InclusionProof::decode(&mut &package.inclusion_proofs[0][..]).expect("summary"),
            InclusionProof::SummaryHeaderStorageV1(_)
        ));
        assert!(matches!(
            InclusionProof::decode(&mut &package.inclusion_proofs[1][..]).expect("export"),
            InclusionProof::ExportV1(_)
        ));
        assert!(matches!(
            InclusionProof::decode(&mut &package.inclusion_proofs[2][..]).expect("import"),
            InclusionProof::FinalizedImportV1(_)
        ));
        assert!(matches!(
            InclusionProof::decode(&mut &package.inclusion_proofs[3][..]).expect("governance"),
            InclusionProof::GovernanceV1(_)
        ));
    }
}
