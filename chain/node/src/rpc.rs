//! A collection of node-specific RPC methods.
#![warn(missing_docs)]

use std::sync::Arc;

use codec::{Decode, Encode};
use frame_support::{storage::storage_prefix, Blake2_128Concat, StorageHasher};
use ialp_common_types::{
    summary_header_storage_key, CertificationPendingReason, GrandpaFinalityCertificate,
    SummaryCertificate, SummaryCertificationBundle, SummaryCertificationReadiness,
    SummaryCertificationState, SummaryHeaderStorageProof, SUMMARY_HEADER_STORAGE_PROOF_VERSION,
};
use jsonrpsee::{
    core::RpcResult,
    types::error::{ErrorCode, ErrorObjectOwned},
    RpcModule,
};
use pallet_ialp_epochs::{SummarySlotRecord, SummarySlotStatus};
use sc_client_api::{Backend as ClientBackend, HeaderBackend, ProofProvider, StorageProvider};
use sp_blockchain::Backend as BlockchainBackend;
use sp_consensus_grandpa::GRANDPA_ENGINE_ID;
use sp_runtime::traits::{BlakeTwo256, Header as HeaderT};
use sp_runtime::Justifications;
use sp_state_machine::read_proof_check;
use sp_storage::{StorageData, StorageKey};
use sp_trie::StorageProof;

use crate::service::{FullBackend, FullClient, FullPool, SharedAuthoritySet};

/// Full client dependencies.
pub struct FullDeps {
    /// The client instance to use.
    pub client: Arc<FullClient>,
    /// Transaction pool instance.
    pub pool: Arc<FullPool>,
    /// The backend is needed for finalized justification lookup.
    pub backend: Arc<FullBackend>,
    /// Shared GRANDPA authority set state is used to resolve the proof set id deterministically.
    pub shared_authority_set: SharedAuthoritySet,
}

#[derive(Clone)]
struct SummaryReadinessRpc {
    client: Arc<FullClient>,
    backend: Arc<FullBackend>,
    shared_authority_set: SharedAuthoritySet,
}

impl SummaryReadinessRpc {
    fn certification_readiness(
        &self,
        epoch_id: ialp_common_types::EpochId,
    ) -> RpcResult<SummaryCertificationReadiness> {
        let best_hash = self.client.info().best_hash;
        let finalized_hash = self.client.info().finalized_hash;
        let finalized_number = self.client.info().finalized_number;

        let slot = self
            .decode_storage_value::<SummarySlotRecord>(
                best_hash,
                summary_slot_storage_key(epoch_id),
            )?
            .ok_or_else(|| {
                invalid_params(format!("epoch {epoch_id} does not have a staged slot"))
            })?;

        if slot.status != SummarySlotStatus::Staged {
            return Err(invalid_params(format!(
                "epoch {epoch_id} is not staged yet (current status: {:?})",
                slot.status
            )));
        }

        let staged_at_block_number = slot.staged_at_block_number.ok_or_else(|| {
            rpc_error(
                ErrorCode::InternalError.code(),
                format!("epoch {epoch_id} staged slot is missing staged_at_block_number"),
            )
        })?;

        let Some(target_block_hash) = self
            .client
            .hash(staged_at_block_number)
            .map_err(client_error)?
        else {
            return Ok(build_certification_readiness(
                epoch_id,
                staged_at_block_number,
                [0u8; 32],
                finalized_number,
                finalized_hash.to_fixed_bytes(),
                SummaryCertificationState::Pending(
                    CertificationPendingReason::MissingTargetBlockHash,
                ),
            ));
        };

        let state = if finalized_number < staged_at_block_number {
            SummaryCertificationState::Pending(CertificationPendingReason::TargetBlockNotFinalized)
        } else if let Some((proof_block_number, proof_block_hash, justification)) =
            self.find_earliest_justified_descendant(staged_at_block_number, finalized_number)?
        {
            let ancestry_headers =
                self.ancestry_headers(staged_at_block_number, proof_block_number)?;
            let resolved = ResolvedProof {
                proof_block_number,
                proof_block_hash,
                justification,
                ancestry_headers,
            };
            let grandpa_set_id = self.grandpa_set_id_for_block(proof_block_number);
            match self.build_summary_certification_bundle(
                epoch_id,
                best_hash,
                staged_at_block_number,
                target_block_hash,
                resolved,
                grandpa_set_id,
            ) {
                Ok(bundle) => SummaryCertificationState::Ready(bundle),
                Err(reason) => SummaryCertificationState::Pending(reason),
            }
        } else {
            SummaryCertificationState::Pending(CertificationPendingReason::NoJustifiedDescendantYet)
        };

        Ok(build_certification_readiness(
            epoch_id,
            staged_at_block_number,
            target_block_hash.to_fixed_bytes(),
            finalized_number,
            finalized_hash.to_fixed_bytes(),
            state,
        ))
    }

    fn decode_storage_value<T: Decode>(
        &self,
        at: <ialp_runtime::opaque::Block as sp_runtime::traits::Block>::Hash,
        key: StorageKey,
    ) -> RpcResult<Option<T>> {
        self.client
            .storage(at, &key)
            .map_err(client_error)?
            .map(|StorageData(bytes)| {
                T::decode(&mut &bytes[..]).map_err(|error| {
                    rpc_error(
                        ErrorCode::InternalError.code(),
                        format!("failed to decode storage value: {error}"),
                    )
                })
            })
            .transpose()
    }

    fn load_storage_bytes(
        &self,
        at: <ialp_runtime::opaque::Block as sp_runtime::traits::Block>::Hash,
        key: StorageKey,
    ) -> Result<Option<Vec<u8>>, sp_blockchain::Error> {
        self.client
            .storage(at, &key)
            .map(|value| value.map(|StorageData(bytes)| bytes))
    }

    fn find_earliest_justified_descendant(
        &self,
        target_block_number: u32,
        finalized_number: u32,
    ) -> RpcResult<Option<(u32, sp_core::H256, Vec<u8>)>> {
        for block_number in target_block_number..=finalized_number {
            let Some(block_hash) = self.client.hash(block_number).map_err(client_error)? else {
                continue;
            };
            let Some(justification) = self.grandpa_justification(block_hash)? else {
                continue;
            };
            return Ok(Some((block_number, block_hash, justification)));
        }

        Ok(None)
    }

    fn grandpa_justification(&self, block_hash: sp_core::H256) -> RpcResult<Option<Vec<u8>>> {
        let Some(justifications) = self
            .backend
            .blockchain()
            .justifications(block_hash)
            .map_err(client_error)?
        else {
            return Ok(None);
        };

        Ok(extract_grandpa_justification(justifications))
    }

    fn ancestry_headers(
        &self,
        target_block_number: u32,
        proof_block_number: u32,
    ) -> RpcResult<Vec<Vec<u8>>> {
        let mut headers = Vec::new();

        for block_number in (target_block_number + 1)..=proof_block_number {
            let Some(block_hash) = self.client.hash(block_number).map_err(client_error)? else {
                return Err(rpc_error(
                    ErrorCode::InternalError.code(),
                    format!("missing canonical block hash for #{block_number}"),
                ));
            };
            let header = self
                .client
                .header(block_hash)
                .map_err(client_error)?
                .ok_or_else(|| {
                    rpc_error(
                        ErrorCode::InternalError.code(),
                        format!("missing canonical header for block #{block_number}"),
                    )
                })?;
            headers.push(header.encode());
        }

        Ok(headers)
    }

    fn build_summary_certification_bundle(
        &self,
        epoch_id: ialp_common_types::EpochId,
        best_hash: <ialp_runtime::opaque::Block as sp_runtime::traits::Block>::Hash,
        target_block_number: u32,
        target_block_hash: sp_core::H256,
        resolved: ResolvedProof,
        grandpa_set_id: u64,
    ) -> Result<SummaryCertificationBundle, CertificationPendingReason> {
        let storage_key = summary_header_storage_key(epoch_id);
        let best_state_value = self
            .load_storage_bytes(best_hash, StorageKey(storage_key.clone()))
            .map_err(proof_pending_reason)?
            .ok_or(CertificationPendingReason::StorageProofConstructionFailed)?;
        let proof_state_value = self
            .load_storage_bytes(resolved.proof_block_hash, StorageKey(storage_key.clone()))
            .map_err(proof_pending_reason)?
            .ok_or(CertificationPendingReason::StorageProofConstructionFailed)?;

        if proof_state_value != best_state_value {
            return Err(CertificationPendingReason::StorageProofConstructionFailed);
        }

        let mut keys = core::iter::once(storage_key.as_slice());
        let storage_proof = self
            .client
            .read_proof(resolved.proof_block_hash, &mut keys)
            .map_err(proof_pending_reason)?;
        let trie_nodes = storage_proof.clone().into_iter_nodes().collect::<Vec<_>>();

        let proof_block_header = self
            .client
            .header(resolved.proof_block_hash)
            .map_err(proof_pending_reason)?
            .ok_or(CertificationPendingReason::StorageProofConstructionFailed)?;
        let proof_block_header_bytes = proof_block_header.encode();

        verify_summary_header_storage_proof_bytes(
            &proof_block_header_bytes,
            resolved.proof_block_hash,
            storage_key.clone(),
            trie_nodes.clone(),
            &best_state_value,
        )?;

        Ok(SummaryCertificationBundle {
            certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                version: ialp_common_types::GRANDPA_FINALITY_CERTIFICATE_VERSION,
                grandpa_set_id,
                target_block_number,
                target_block_hash: target_block_hash.to_fixed_bytes(),
                proof_block_number: resolved.proof_block_number,
                proof_block_hash: resolved.proof_block_hash.to_fixed_bytes(),
                justification: resolved.justification,
                ancestry_headers: resolved.ancestry_headers,
            }),
            summary_header_storage_proof: SummaryHeaderStorageProof {
                version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                proof_block_number: resolved.proof_block_number,
                proof_block_hash: resolved.proof_block_hash.to_fixed_bytes(),
                proof_block_header: proof_block_header_bytes,
                storage_key,
                trie_nodes,
            },
        })
    }

    fn grandpa_set_id_for_block(&self, block_number: u32) -> u64 {
        let authority_set_changes = self.shared_authority_set.authority_set_changes();
        let changes = authority_set_changes
            .iter_from(0)
            .map(|changes| {
                changes
                    .map(|(set_id, last)| (*set_id, *last))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        resolve_grandpa_set_id(self.shared_authority_set.set_id(), &changes, block_number)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedProof {
    proof_block_number: u32,
    proof_block_hash: sp_core::H256,
    justification: Vec<u8>,
    ancestry_headers: Vec<Vec<u8>>,
}

/// Instantiate all full RPC extensions.
pub fn create_full(
    deps: FullDeps,
) -> Result<RpcModule<()>, Box<dyn std::error::Error + Send + Sync>> {
    use pallet_transaction_payment_rpc::{TransactionPayment, TransactionPaymentApiServer};
    use substrate_frame_rpc_system::{System, SystemApiServer};

    let mut module = RpcModule::new(());
    let FullDeps {
        client,
        pool,
        backend,
        shared_authority_set,
    } = deps;

    module.merge(System::new(client.clone(), pool).into_rpc())?;
    module.merge(TransactionPayment::new(client.clone()).into_rpc())?;

    // Phase 2A keeps summary headers on normal storage queries and evolves the single proof-aware
    // readiness RPC so it can return GRANDPA context plus the canonical storage proof bundle.
    let summary_rpc = SummaryReadinessRpc {
        client,
        backend,
        shared_authority_set,
    };
    module.register_method(
        "ialp_summary_certificationReadiness",
        move |params, _, _| {
            let epoch_id: ialp_common_types::EpochId = params.one()?;
            summary_rpc.certification_readiness(epoch_id)
        },
    )?;

    Ok(module)
}

fn summary_slot_storage_key(epoch_id: ialp_common_types::EpochId) -> StorageKey {
    let mut key = storage_prefix(b"Epochs", b"SummarySlots").to_vec();
    key.extend(Blake2_128Concat::hash(&epoch_id.encode()));
    StorageKey(key)
}

fn extract_grandpa_justification(justifications: Justifications) -> Option<Vec<u8>> {
    justifications.into_justification(GRANDPA_ENGINE_ID)
}

fn build_certification_readiness(
    epoch_id: ialp_common_types::EpochId,
    staged_at_block_number: u32,
    staged_at_block_hash: [u8; 32],
    latest_finalized_block_number: u32,
    latest_finalized_block_hash: [u8; 32],
    state: SummaryCertificationState,
) -> SummaryCertificationReadiness {
    SummaryCertificationReadiness {
        epoch_id,
        staged_at_block_number,
        staged_at_block_hash,
        latest_finalized_block_number,
        latest_finalized_block_hash,
        state,
    }
}

fn verify_summary_header_storage_proof_bytes(
    proof_block_header_bytes: &[u8],
    proof_block_hash: sp_core::H256,
    storage_key: Vec<u8>,
    trie_nodes: Vec<Vec<u8>>,
    expected_value: &[u8],
) -> Result<(), CertificationPendingReason> {
    let decoded_header = ialp_runtime::Header::decode(&mut &proof_block_header_bytes[..])
        .map_err(|_| CertificationPendingReason::StorageProofConstructionFailed)?;
    if decoded_header.hash() != proof_block_hash {
        return Err(CertificationPendingReason::StorageProofConstructionFailed);
    }

    let verified_values = read_proof_check::<BlakeTwo256, _>(
        *decoded_header.state_root(),
        StorageProof::new(trie_nodes),
        [storage_key.as_slice()],
    )
    .map_err(|_| CertificationPendingReason::StorageProofConstructionFailed)?;

    match verified_values.get(&storage_key) {
        Some(Some(value)) if value == expected_value => Ok(()),
        _ => Err(CertificationPendingReason::StorageProofConstructionFailed),
    }
}

fn proof_pending_reason(error: sp_blockchain::Error) -> CertificationPendingReason {
    if is_historical_state_unavailable(&error) {
        CertificationPendingReason::HistoricalStateUnavailable
    } else {
        CertificationPendingReason::StorageProofConstructionFailed
    }
}

fn is_historical_state_unavailable(error: &sp_blockchain::Error) -> bool {
    match error {
        sp_blockchain::Error::UnknownBlock(_)
        | sp_blockchain::Error::InvalidState
        | sp_blockchain::Error::StateDatabase(_)
        | sp_blockchain::Error::MissingHeader(_) => true,
        sp_blockchain::Error::Blockchain(inner) => is_historical_state_unavailable(inner),
        _ => false,
    }
}

fn resolve_grandpa_set_id(current_set_id: u64, changes: &[(u64, u32)], block_number: u32) -> u64 {
    for (set_id, last_block_for_set) in changes {
        if block_number <= *last_block_for_set {
            return *set_id;
        }
    }

    current_set_id
}

fn invalid_params(message: impl Into<String>) -> ErrorObjectOwned {
    rpc_error(ErrorCode::InvalidParams.code(), message)
}

fn client_error(error: impl core::fmt::Display) -> ErrorObjectOwned {
    rpc_error(ErrorCode::InternalError.code(), error.to_string())
}

fn rpc_error(code: i32, message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(code, message.into(), None::<()>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ialp_common_types::{
        summary_header_storage_key, EpochSummaryHeader, SummaryCertificate,
        SummaryHeaderStorageProof, EMPTY_HASH, SUMMARY_HEADER_STORAGE_PROOF_VERSION,
    };
    use sp_blockchain::Error as BlockchainError;
    use sp_runtime::Digest;
    use sp_state_machine::{prove_read, Backend as _, TestExternalities};
    use sp_storage::Storage;

    #[test]
    fn readiness_is_pending_before_target_block_finalizes() {
        let readiness = build_certification_readiness(
            7,
            10,
            [1u8; 32],
            9,
            [2u8; 32],
            SummaryCertificationState::Pending(CertificationPendingReason::TargetBlockNotFinalized),
        );

        assert_eq!(
            readiness.state,
            SummaryCertificationState::Pending(CertificationPendingReason::TargetBlockNotFinalized)
        );
    }

    #[test]
    fn readiness_is_pending_without_justified_descendant_in_finalized_range() {
        let readiness = build_certification_readiness(
            7,
            10,
            [1u8; 32],
            12,
            [2u8; 32],
            SummaryCertificationState::Pending(
                CertificationPendingReason::NoJustifiedDescendantYet,
            ),
        );

        assert_eq!(
            readiness.state,
            SummaryCertificationState::Pending(
                CertificationPendingReason::NoJustifiedDescendantYet
            )
        );
    }

    #[test]
    fn readiness_is_ready_with_deterministic_proof_content() {
        let bundle = SummaryCertificationBundle {
            certificate: SummaryCertificate::GrandpaV1(GrandpaFinalityCertificate {
                version: ialp_common_types::GRANDPA_FINALITY_CERTIFICATE_VERSION,
                grandpa_set_id: 1,
                target_block_number: 10,
                target_block_hash: [1u8; 32],
                proof_block_number: 12,
                proof_block_hash: [3u8; 32],
                justification: vec![1, 2, 3],
                ancestry_headers: vec![vec![4, 5]],
            }),
            summary_header_storage_proof: SummaryHeaderStorageProof {
                version: SUMMARY_HEADER_STORAGE_PROOF_VERSION,
                proof_block_number: 12,
                proof_block_hash: [3u8; 32],
                proof_block_header: vec![9, 9, 9],
                storage_key: summary_header_storage_key(7),
                trie_nodes: vec![vec![1, 2]],
            },
        };
        let first = build_certification_readiness(
            7,
            10,
            [1u8; 32],
            12,
            [2u8; 32],
            SummaryCertificationState::Ready(bundle.clone()),
        );
        let second = build_certification_readiness(
            7,
            10,
            [1u8; 32],
            12,
            [2u8; 32],
            SummaryCertificationState::Ready(bundle),
        );

        assert_eq!(first, second);
        match first.state {
            SummaryCertificationState::Ready(bundle) => {
                let SummaryCertificate::GrandpaV1(certificate) = &bundle.certificate;
                assert_eq!(certificate.proof_block_number, 12);
                assert_eq!(certificate.justification, vec![1, 2, 3]);
                assert_eq!(
                    bundle.summary_header_storage_proof.storage_key,
                    summary_header_storage_key(7)
                );
            }
            _ => panic!("expected ready state"),
        }
    }

    #[test]
    fn generated_summary_header_storage_proof_verifies_for_canonical_key() {
        let epoch_id = 7;
        let storage_key = summary_header_storage_key(epoch_id);
        let header = EpochSummaryHeader {
            version: 1,
            domain_id: ialp_common_types::DomainId::Earth,
            epoch_id,
            prev_summary_hash: EMPTY_HASH,
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
        };

        let mut storage = Storage::default();
        storage.top.insert(storage_key.clone(), header.encode());
        let ext = TestExternalities::<BlakeTwo256>::new(storage);
        let state_root = ext
            .backend
            .storage_root(std::iter::empty(), ext.state_version)
            .0;
        let storage_proof = prove_read(ext.backend.clone(), &[storage_key.as_slice()])
            .expect("proof generation should succeed");
        let trie_nodes = storage_proof.clone().into_iter_nodes().collect::<Vec<_>>();
        let proof_block_header = ialp_runtime::Header::new(
            11,
            EMPTY_HASH.into(),
            state_root,
            EMPTY_HASH.into(),
            Digest::default(),
        );

        verify_summary_header_storage_proof_bytes(
            &proof_block_header.encode(),
            proof_block_header.hash(),
            storage_key,
            trie_nodes,
            &header.encode(),
        )
        .expect("proof should verify");
    }

    #[test]
    fn generated_summary_header_storage_proof_rejects_wrong_key() {
        let epoch_id = 7;
        let correct_key = summary_header_storage_key(epoch_id);
        let wrong_key = summary_header_storage_key(epoch_id + 1);
        let header = EpochSummaryHeader {
            version: 1,
            domain_id: ialp_common_types::DomainId::Earth,
            epoch_id,
            prev_summary_hash: EMPTY_HASH,
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
        };

        let mut storage = Storage::default();
        storage.top.insert(correct_key.clone(), header.encode());
        let ext = TestExternalities::<BlakeTwo256>::new(storage);
        let state_root = ext
            .backend
            .storage_root(std::iter::empty(), ext.state_version)
            .0;
        let storage_proof = prove_read(ext.backend.clone(), &[correct_key.as_slice()])
            .expect("proof generation should succeed");
        let trie_nodes = storage_proof.into_iter_nodes().collect::<Vec<_>>();
        let proof_block_header = ialp_runtime::Header::new(
            11,
            EMPTY_HASH.into(),
            state_root,
            EMPTY_HASH.into(),
            Digest::default(),
        );

        let error = verify_summary_header_storage_proof_bytes(
            &proof_block_header.encode(),
            proof_block_header.hash(),
            wrong_key,
            trie_nodes,
            &header.encode(),
        )
        .expect_err("wrong key should fail verification");

        assert_eq!(
            error,
            CertificationPendingReason::StorageProofConstructionFailed
        );
    }

    #[test]
    fn historical_state_errors_map_to_pending_reason() {
        assert_eq!(
            proof_pending_reason(BlockchainError::UnknownBlock("missing".into())),
            CertificationPendingReason::HistoricalStateUnavailable
        );
        assert_eq!(
            proof_pending_reason(BlockchainError::StateDatabase("pruned".into())),
            CertificationPendingReason::HistoricalStateUnavailable
        );
    }

    #[test]
    fn non_historical_proof_errors_map_to_construction_failure() {
        assert_eq!(
            proof_pending_reason(BlockchainError::Storage("boom".into())),
            CertificationPendingReason::StorageProofConstructionFailed
        );
    }

    #[test]
    fn grandpa_set_resolution_uses_historical_changes_before_current_set() {
        let changes = vec![(0, 15), (1, 40)];

        assert_eq!(resolve_grandpa_set_id(2, &changes, 10), 0);
        assert_eq!(resolve_grandpa_set_id(2, &changes, 20), 1);
        assert_eq!(resolve_grandpa_set_id(2, &changes, 41), 2);
    }
}
