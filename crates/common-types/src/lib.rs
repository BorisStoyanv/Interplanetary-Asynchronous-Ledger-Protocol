#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{vec, vec::Vec};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::{fmt, str::FromStr};
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
pub const SUMMARY_HEADERS_PROOF_INDEX: usize = 0;

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

pub fn summary_header_storage_key(epoch_id: EpochId) -> Vec<u8> {
    let encoded_epoch = epoch_id.encode();
    let mut key = Vec::with_capacity(32 + 16 + encoded_epoch.len());
    key.extend_from_slice(&twox_128(b"Epochs"));
    key.extend_from_slice(&twox_128(b"SummaryHeaders"));
    key.extend_from_slice(&blake2_128(&encoded_epoch));
    key.extend_from_slice(&encoded_epoch);
    key
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

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum InclusionProof {
    SummaryHeaderStorageV1(SummaryHeaderStorageProof),
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
        Self::from_fields(
            header,
            bundle.certificate,
            vec![
                InclusionProof::SummaryHeaderStorageV1(bundle.summary_header_storage_proof)
                    .encode(),
            ],
            Vec::new(),
        )
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
}
