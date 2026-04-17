#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use frame_support::{
        pallet_prelude::*,
        traits::fungible::Inspect,
    };
    use frame_system::pallet_prelude::*;
    use ialp_common_types::{
        fixed_bytes, governance_merkle_root, governance_proposal_id, AccountIdBytes, DomainId,
        EpochId, GovernanceAckLeaf, GovernanceAckLeafHashInput, GovernanceAckRecord,
        GovernanceActivationRecord, GovernanceLeaf, GovernancePayload, GovernanceProposal,
        GovernanceProposalId, GovernanceProposalLeaf, GovernanceProposalLeafHashInput,
        GovernanceProposalStatus, GovernanceVote, GovernanceVoteChoice, GOVERNANCE_ACK_LEAF_VERSION,
        GOVERNANCE_ACK_RECORD_VERSION, GOVERNANCE_ACTIVATION_RECORD_VERSION,
        GOVERNANCE_PROPOSAL_LEAF_VERSION, GOVERNANCE_PROPOSAL_VERSION, GOVERNANCE_VOTE_VERSION,
    };
    use sp_runtime::traits::SaturatedConversion;
    use sp_std::{vec, vec::Vec};

    pub trait DomainIdentityProvider {
        fn domain_id() -> DomainId;
    }

    pub trait EpochInfoProvider {
        fn current_epoch() -> EpochId;
    }

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: Inspect<Self::AccountId, Balance = u128>;
        type DomainIdentity: DomainIdentityProvider;
        type EpochInfo: EpochInfoProvider;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::storage]
    #[pallet::getter(fn protocol_version)]
    pub type ProtocolVersion<T> = StorageValue<_, u32, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn importer_account)]
    pub type ImporterAccount<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;

    #[pallet::storage]
    #[pallet::getter(fn governance_voters)]
    #[pallet::unbounded]
    pub type GovernanceVoters<T: Config> = StorageValue<_, Vec<T::AccountId>, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn next_proposal_sequence)]
    pub type NextProposalSequence<T> = StorageValue<_, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn proposal)]
    #[pallet::unbounded]
    pub type ProposalsById<T> =
        StorageMap<_, Blake2_128Concat, GovernanceProposalId, GovernanceProposal, OptionQuery>;

    #[pallet::storage]
    #[pallet::getter(fn vote)]
    #[pallet::unbounded]
    pub type VotesByKey<T> = StorageMap<
        _,
        Blake2_128Concat,
        (GovernanceProposalId, AccountIdBytes),
        GovernanceVote,
        OptionQuery,
    >;

    #[pallet::storage]
    pub type ProposalVotingPowerByVoter<T> = StorageMap<
        _,
        Blake2_128Concat,
        (GovernanceProposalId, AccountIdBytes),
        u128,
        OptionQuery,
    >;

    #[pallet::storage]
    #[pallet::getter(fn activation_record)]
    #[pallet::unbounded]
    pub type ActivationRecordsById<T> = StorageMap<
        _,
        Blake2_128Concat,
        GovernanceProposalId,
        GovernanceActivationRecord,
        OptionQuery,
    >;

    #[pallet::storage]
    #[pallet::getter(fn ack_record)]
    #[pallet::unbounded]
    pub type AckRecordsByKey<T> = StorageMap<
        _,
        Blake2_128Concat,
        (GovernanceProposalId, DomainId),
        GovernanceAckRecord,
        OptionQuery,
    >;

    #[pallet::storage]
    #[pallet::getter(fn epoch_governance_leaf_ids)]
    #[pallet::unbounded]
    pub type EpochGovernanceLeafIds<T> =
        StorageMap<_, Blake2_128Concat, EpochId, Vec<[u8; 32]>, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn governance_leaf)]
    #[pallet::unbounded]
    pub type GovernanceLeavesById<T> =
        StorageMap<_, Blake2_128Concat, [u8; 32], GovernanceLeaf, OptionQuery>;

    #[pallet::storage]
    #[pallet::unbounded]
    pub type ScheduledProposalIdsByEpoch<T> =
        StorageMap<_, Blake2_128Concat, EpochId, Vec<GovernanceProposalId>, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn ready_activation_ids)]
    #[pallet::unbounded]
    pub type ReadyActivationIds<T> = StorageValue<_, Vec<GovernanceProposalId>, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub protocol_version: u32,
        pub importer_account: Option<T::AccountId>,
        pub governance_voters: Vec<T::AccountId>,
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                protocol_version: 1,
                importer_account: None,
                governance_voters: Vec::new(),
                _marker: Default::default(),
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            ProtocolVersion::<T>::put(self.protocol_version.max(1));
            if let Some(account) = &self.importer_account {
                ImporterAccount::<T>::put(account);
            }
            let mut voters = self.governance_voters.clone();
            voters.sort();
            voters.dedup();
            GovernanceVoters::<T>::put(voters);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        ProposalCreated {
            proposal_id: GovernanceProposalId,
            source_domain: DomainId,
            target_domains: Vec<DomainId>,
            voting_end_epoch: EpochId,
        },
        VoteRecorded {
            proposal_id: GovernanceProposalId,
            voter: AccountIdBytes,
            choice: GovernanceVoteChoice,
            voting_power: u128,
        },
        ProposalRejected {
            proposal_id: GovernanceProposalId,
            epoch_id: EpochId,
        },
        ProposalLocallyFinalized {
            proposal_id: GovernanceProposalId,
            approval_epoch: EpochId,
            activation_epoch: EpochId,
        },
        ProposalImported {
            proposal_id: GovernanceProposalId,
            source_domain: DomainId,
            target_domain: DomainId,
        },
        GovernanceAcknowledged {
            proposal_id: GovernanceProposalId,
            acknowledging_domain: DomainId,
            local_domain: DomainId,
        },
        ProposalScheduled {
            proposal_id: GovernanceProposalId,
            activation_epoch: EpochId,
        },
        ProposalActivated {
            proposal_id: GovernanceProposalId,
            activation_epoch: EpochId,
            new_protocol_version: u32,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        NoTargetDomains,
        DuplicateTargetDomains,
        SameDomainTarget,
        InvalidProtocolVersion,
        ProposalMissing,
        ProposalAlreadyExists,
        ProposalNotVoting,
        VotingClosed,
        VotingStillOpen,
        DuplicateVote,
        NotEligibleVoter,
        ZeroVotingPower,
        MissingImporterAccount,
        UnauthorizedImporter,
        WrongTargetDomain,
        WrongSourceDomain,
        DuplicateImportedProposal,
        DuplicateImportedAck,
        ProposalNotKnown,
        ProposalFactsMismatch,
        AckingDomainNotTargeted,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(block_number: BlockNumberFor<T>) -> Weight {
            if block_number == BlockNumberFor::<T>::zero() {
                return Weight::zero();
            }

            let current_epoch = T::EpochInfo::current_epoch();
            let block_height: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            Self::activate_epoch_bucket(current_epoch, block_height);
            Self::activate_ready_queue(current_epoch, block_height);
            Weight::zero()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::call_index(0)]
        #[pallet::weight(10_000)]
        pub fn create_proposal(
            origin: OriginFor<T>,
            mut target_domains: Vec<DomainId>,
            new_protocol_version: u32,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let source_domain = T::DomainIdentity::domain_id();
            target_domains = Self::canonicalize_target_domains(target_domains, source_domain)?;

            let current_version = ProtocolVersion::<T>::get();
            ensure!(
                new_protocol_version > 0 && new_protocol_version != current_version,
                Error::<T>::InvalidProtocolVersion
            );

            let proposal_sequence = NextProposalSequence::<T>::get();
            NextProposalSequence::<T>::put(proposal_sequence.saturating_add(1));

            let proposal_id = governance_proposal_id(source_domain, proposal_sequence);
            let current_epoch = T::EpochInfo::current_epoch();
            let voting_end_epoch = current_epoch.saturating_add(1);
            let payload = GovernancePayload::SetProtocolVersion {
                new_version: new_protocol_version,
            };
            let payload_hash = payload.payload_hash();
            let proposer = Self::account_to_bytes(&who);

            let mut snapshot_total_voting_power = 0u128;
            for voter in GovernanceVoters::<T>::get() {
                let voter_bytes = Self::account_to_bytes(&voter);
                let balance = T::Currency::balance(&voter);
                ProposalVotingPowerByVoter::<T>::insert((proposal_id, voter_bytes), balance);
                snapshot_total_voting_power = snapshot_total_voting_power.saturating_add(balance);
            }

            let proposal = GovernanceProposal {
                version: GOVERNANCE_PROPOSAL_VERSION,
                proposal_id,
                source_domain,
                target_domains: target_domains.clone(),
                proposer,
                payload,
                payload_hash,
                created_epoch: current_epoch,
                voting_start_epoch: current_epoch,
                voting_end_epoch,
                approval_epoch: None,
                activation_epoch: 0,
                snapshot_total_voting_power,
                quorum_numerator: 1,
                quorum_denominator: 2,
                yes_voting_power: 0,
                no_voting_power: 0,
                abstain_voting_power: 0,
                status: GovernanceProposalStatus::Voting,
            };

            ProposalsById::<T>::insert(proposal_id, proposal);
            Self::deposit_event(Event::ProposalCreated {
                proposal_id,
                source_domain,
                target_domains,
                voting_end_epoch,
            });
            Ok(())
        }

        #[pallet::call_index(1)]
        #[pallet::weight(10_000)]
        pub fn cast_vote(
            origin: OriginFor<T>,
            proposal_id: GovernanceProposalId,
            choice: GovernanceVoteChoice,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let voter = Self::account_to_bytes(&who);
            let current_epoch = T::EpochInfo::current_epoch();

            let mut proposal =
                ProposalsById::<T>::get(proposal_id).ok_or(Error::<T>::ProposalMissing)?;
            ensure!(
                proposal.status == GovernanceProposalStatus::Voting,
                Error::<T>::ProposalNotVoting
            );
            ensure!(
                current_epoch < proposal.voting_end_epoch,
                Error::<T>::VotingClosed
            );
            ensure!(
                !VotesByKey::<T>::contains_key((proposal_id, voter)),
                Error::<T>::DuplicateVote
            );
            let voting_power = ProposalVotingPowerByVoter::<T>::get((proposal_id, voter))
                .ok_or(Error::<T>::NotEligibleVoter)?;
            ensure!(voting_power > 0, Error::<T>::ZeroVotingPower);

            let block_height: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            VotesByKey::<T>::insert(
                (proposal_id, voter),
                GovernanceVote {
                    version: GOVERNANCE_VOTE_VERSION,
                    proposal_id,
                    voter,
                    choice,
                    voting_power,
                    cast_epoch: current_epoch,
                    cast_block_height: block_height,
                },
            );

            match choice {
                GovernanceVoteChoice::Yes => {
                    proposal.yes_voting_power =
                        proposal.yes_voting_power.saturating_add(voting_power);
                }
                GovernanceVoteChoice::No => {
                    proposal.no_voting_power =
                        proposal.no_voting_power.saturating_add(voting_power);
                }
                GovernanceVoteChoice::Abstain => {
                    proposal.abstain_voting_power =
                        proposal.abstain_voting_power.saturating_add(voting_power);
                }
            }
            ProposalsById::<T>::insert(proposal_id, proposal);

            Self::deposit_event(Event::VoteRecorded {
                proposal_id,
                voter,
                choice,
                voting_power,
            });
            Ok(())
        }

        #[pallet::call_index(2)]
        #[pallet::weight(10_000)]
        pub fn close_proposal(
            origin: OriginFor<T>,
            proposal_id: GovernanceProposalId,
        ) -> DispatchResult {
            let _ = ensure_signed(origin)?;
            let current_epoch = T::EpochInfo::current_epoch();
            let mut proposal =
                ProposalsById::<T>::get(proposal_id).ok_or(Error::<T>::ProposalMissing)?;
            ensure!(
                proposal.status == GovernanceProposalStatus::Voting,
                Error::<T>::ProposalNotVoting
            );
            ensure!(
                current_epoch >= proposal.voting_end_epoch,
                Error::<T>::VotingStillOpen
            );

            let recorded_voting_power = proposal
                .yes_voting_power
                .saturating_add(proposal.no_voting_power)
                .saturating_add(proposal.abstain_voting_power);
            let quorum_met = recorded_voting_power
                .saturating_mul(u128::from(proposal.quorum_denominator))
                >= proposal
                    .snapshot_total_voting_power
                    .saturating_mul(u128::from(proposal.quorum_numerator));
            let approved =
                proposal.yes_voting_power.saturating_mul(2) > recorded_voting_power;

            if !quorum_met || !approved {
                proposal.status = GovernanceProposalStatus::Rejected;
                ProposalsById::<T>::insert(proposal_id, proposal);
                Self::deposit_event(Event::ProposalRejected {
                    proposal_id,
                    epoch_id: current_epoch,
                });
                return Ok(());
            }

            let activation_epoch = current_epoch.saturating_add(4);
            proposal.approval_epoch = Some(current_epoch);
            proposal.activation_epoch = activation_epoch;
            proposal.status = GovernanceProposalStatus::LocallyFinalized;
            Self::store_activation_record(&proposal, Vec::new(), current_epoch, None);
            Self::insert_outbound_proposal_leaves(&proposal, current_epoch);
            ProposalsById::<T>::insert(proposal_id, proposal);

            Self::deposit_event(Event::ProposalLocallyFinalized {
                proposal_id,
                approval_epoch: current_epoch,
                activation_epoch,
            });
            Ok(())
        }

        #[pallet::call_index(3)]
        #[pallet::weight(10_000)]
        pub fn import_verified_governance_proposal(
            origin: OriginFor<T>,
            claim: ialp_common_types::ImportedGovernanceProposalClaim,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let configured_importer =
                ImporterAccount::<T>::get().ok_or(Error::<T>::MissingImporterAccount)?;
            ensure!(who == configured_importer, Error::<T>::UnauthorizedImporter);

            let local_domain = T::DomainIdentity::domain_id();
            let leaf = claim.leaf;
            ensure!(leaf.target_domain == local_domain, Error::<T>::WrongTargetDomain);
            ensure!(leaf.source_domain != local_domain, Error::<T>::WrongSourceDomain);
            ensure!(
                !ProposalsById::<T>::contains_key(leaf.proposal_id),
                Error::<T>::DuplicateImportedProposal
            );
            ensure!(
                leaf.target_domains.contains(&local_domain),
                Error::<T>::WrongTargetDomain
            );

            let payload = GovernancePayload::SetProtocolVersion {
                new_version: leaf.new_protocol_version,
            };
            ensure!(
                payload.payload_hash() == leaf.payload_hash,
                Error::<T>::ProposalFactsMismatch
            );

            let proposal = GovernanceProposal {
                version: GOVERNANCE_PROPOSAL_VERSION,
                proposal_id: leaf.proposal_id,
                source_domain: leaf.source_domain,
                target_domains: leaf.target_domains.clone(),
                proposer: leaf.proposer,
                payload,
                payload_hash: leaf.payload_hash,
                created_epoch: leaf.created_epoch,
                voting_start_epoch: leaf.voting_start_epoch,
                voting_end_epoch: leaf.voting_end_epoch,
                approval_epoch: Some(leaf.approval_epoch),
                activation_epoch: leaf.activation_epoch,
                snapshot_total_voting_power: 0,
                quorum_numerator: 1,
                quorum_denominator: 2,
                yes_voting_power: 0,
                no_voting_power: 0,
                abstain_voting_power: 0,
                status: GovernanceProposalStatus::LocallyFinalized,
            };
            let current_epoch = T::EpochInfo::current_epoch();
            ProposalsById::<T>::insert(leaf.proposal_id, &proposal);
            Self::store_activation_record(
                &proposal,
                vec![local_domain],
                current_epoch,
                None,
            );
            Self::record_local_ack_and_fanout(&proposal, current_epoch);
            Self::maybe_schedule_activation(leaf.proposal_id, current_epoch);

            Self::deposit_event(Event::ProposalImported {
                proposal_id: leaf.proposal_id,
                source_domain: leaf.source_domain,
                target_domain: local_domain,
            });
            Ok(())
        }

        #[pallet::call_index(4)]
        #[pallet::weight(10_000)]
        pub fn import_verified_governance_ack(
            origin: OriginFor<T>,
            claim: ialp_common_types::ImportedGovernanceAckClaim,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let configured_importer =
                ImporterAccount::<T>::get().ok_or(Error::<T>::MissingImporterAccount)?;
            ensure!(who == configured_importer, Error::<T>::UnauthorizedImporter);

            let local_domain = T::DomainIdentity::domain_id();
            let leaf = claim.leaf;
            ensure!(leaf.target_domain == local_domain, Error::<T>::WrongTargetDomain);
            ensure!(
                leaf.acknowledging_domain != local_domain,
                Error::<T>::WrongSourceDomain
            );
            ensure!(
                leaf.target_domains.contains(&leaf.acknowledging_domain),
                Error::<T>::AckingDomainNotTargeted
            );
            ensure!(
                !AckRecordsByKey::<T>::contains_key((leaf.proposal_id, leaf.acknowledging_domain)),
                Error::<T>::DuplicateImportedAck
            );

            let proposal =
                ProposalsById::<T>::get(leaf.proposal_id).ok_or(Error::<T>::ProposalNotKnown)?;
            ensure!(
                proposal.source_domain == leaf.source_domain
                    && proposal.target_domains == leaf.target_domains
                    && proposal.payload_hash == leaf.payload_hash
                    && proposal.payload.protocol_version() == leaf.new_protocol_version
                    && proposal.activation_epoch == leaf.activation_epoch,
                Error::<T>::ProposalFactsMismatch
            );

            let block_height: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            let current_epoch = T::EpochInfo::current_epoch();
            AckRecordsByKey::<T>::insert(
                (leaf.proposal_id, leaf.acknowledging_domain),
                GovernanceAckRecord {
                    version: GOVERNANCE_ACK_RECORD_VERSION,
                    proposal_id: leaf.proposal_id,
                    source_domain: leaf.source_domain,
                    acknowledging_domain: leaf.acknowledging_domain,
                    target_domains: leaf.target_domains.clone(),
                    activation_epoch: leaf.activation_epoch,
                    payload_hash: leaf.payload_hash,
                    new_protocol_version: leaf.new_protocol_version,
                    acknowledged_epoch: leaf.acknowledged_epoch,
                    acknowledged_at_local_block_height: block_height,
                },
            );
            Self::note_known_ack_domain(leaf.proposal_id, leaf.acknowledging_domain, current_epoch);

            Self::deposit_event(Event::GovernanceAcknowledged {
                proposal_id: leaf.proposal_id,
                acknowledging_domain: leaf.acknowledging_domain,
                local_domain,
            });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn canonical_epoch_governance_leaves(epoch_id: EpochId) -> Vec<GovernanceLeaf> {
            let mut leaves = EpochGovernanceLeafIds::<T>::get(epoch_id)
                .into_iter()
                .filter_map(|leaf_id| GovernanceLeavesById::<T>::get(leaf_id))
                .collect::<Vec<_>>();
            ialp_common_types::sort_governance_leaves(&mut leaves);
            leaves
        }

        pub fn commit_epoch_governance(
            epoch_id: EpochId,
            start_block_height: u32,
            end_block_height: u32,
        ) -> [u8; 32] {
            let domain_id = T::DomainIdentity::domain_id();
            let leaves = Self::canonical_epoch_governance_leaves(epoch_id);
            governance_merkle_root(domain_id, epoch_id, start_block_height, end_block_height, &leaves)
        }

        pub fn account_to_bytes(account: &T::AccountId) -> AccountIdBytes {
            fixed_bytes(&account.encode())
        }

        fn store_activation_record(
            proposal: &GovernanceProposal,
            mut known_ack_domains: Vec<DomainId>,
            current_epoch: EpochId,
            activated_at_block_height: Option<u32>,
        ) {
            known_ack_domains.sort();
            known_ack_domains.dedup();
            ActivationRecordsById::<T>::insert(
                proposal.proposal_id,
                GovernanceActivationRecord {
                    version: GOVERNANCE_ACTIVATION_RECORD_VERSION,
                    proposal_id: proposal.proposal_id,
                    source_domain: proposal.source_domain,
                    target_domains: proposal.target_domains.clone(),
                    payload_hash: proposal.payload_hash,
                    new_protocol_version: proposal.payload.protocol_version(),
                    activation_epoch: proposal.activation_epoch,
                    known_ack_domains,
                    scheduled_at_epoch: None,
                    activated_at_epoch: proposal
                        .approval_epoch
                        .filter(|_| proposal.status == GovernanceProposalStatus::Activated),
                    activated_at_local_block_height: activated_at_block_height,
                    status: proposal.status,
                },
            );
            let _ = current_epoch;
        }

        fn canonicalize_target_domains(
            mut target_domains: Vec<DomainId>,
            source_domain: DomainId,
        ) -> Result<Vec<DomainId>, Error<T>> {
            ensure!(!target_domains.is_empty(), Error::<T>::NoTargetDomains);
            if target_domains.contains(&source_domain) {
                return Err(Error::<T>::SameDomainTarget);
            }
            target_domains.sort();
            let original_len = target_domains.len();
            target_domains.dedup();
            ensure!(
                target_domains.len() == original_len,
                Error::<T>::DuplicateTargetDomains
            );
            Ok(target_domains)
        }

        fn insert_outbound_proposal_leaves(proposal: &GovernanceProposal, approval_epoch: EpochId) {
            let approval_epoch = proposal.approval_epoch.unwrap_or(approval_epoch);
            for target_domain in &proposal.target_domains {
                let leaf = GovernanceLeaf::ProposalV1(GovernanceProposalLeaf::from_hash_input(
                    GovernanceProposalLeafHashInput {
                        version: GOVERNANCE_PROPOSAL_LEAF_VERSION,
                        proposal_id: proposal.proposal_id,
                        source_domain: proposal.source_domain,
                        target_domain: *target_domain,
                        target_domains: proposal.target_domains.clone(),
                        proposer: proposal.proposer,
                        payload_hash: proposal.payload_hash,
                        new_protocol_version: proposal.payload.protocol_version(),
                        created_epoch: proposal.created_epoch,
                        voting_start_epoch: proposal.voting_start_epoch,
                        voting_end_epoch: proposal.voting_end_epoch,
                        approval_epoch,
                        activation_epoch: proposal.activation_epoch,
                    },
                ));
                Self::insert_epoch_leaf(approval_epoch, leaf);
            }
        }

        fn record_local_ack_and_fanout(proposal: &GovernanceProposal, current_epoch: EpochId) {
            let local_domain = T::DomainIdentity::domain_id();
            let block_height: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            AckRecordsByKey::<T>::insert(
                (proposal.proposal_id, local_domain),
                GovernanceAckRecord {
                    version: GOVERNANCE_ACK_RECORD_VERSION,
                    proposal_id: proposal.proposal_id,
                    source_domain: proposal.source_domain,
                    acknowledging_domain: local_domain,
                    target_domains: proposal.target_domains.clone(),
                    activation_epoch: proposal.activation_epoch,
                    payload_hash: proposal.payload_hash,
                    new_protocol_version: proposal.payload.protocol_version(),
                    acknowledged_epoch: current_epoch,
                    acknowledged_at_local_block_height: block_height,
                },
            );

            let mut fanout_targets = proposal.target_domains.clone();
            fanout_targets.push(proposal.source_domain);
            fanout_targets.sort();
            fanout_targets.dedup();
            fanout_targets.retain(|domain| domain != &local_domain);

            for target_domain in fanout_targets {
                let leaf = GovernanceLeaf::AckV1(GovernanceAckLeaf::from_hash_input(
                    GovernanceAckLeafHashInput {
                        version: GOVERNANCE_ACK_LEAF_VERSION,
                        proposal_id: proposal.proposal_id,
                        source_domain: proposal.source_domain,
                        target_domain,
                        acknowledging_domain: local_domain,
                        target_domains: proposal.target_domains.clone(),
                        payload_hash: proposal.payload_hash,
                        new_protocol_version: proposal.payload.protocol_version(),
                        activation_epoch: proposal.activation_epoch,
                        acknowledged_epoch: current_epoch,
                    },
                ));
                Self::insert_epoch_leaf(current_epoch, leaf);
            }

            Self::deposit_event(Event::GovernanceAcknowledged {
                proposal_id: proposal.proposal_id,
                acknowledging_domain: local_domain,
                local_domain,
            });
        }

        fn insert_epoch_leaf(epoch_id: EpochId, leaf: GovernanceLeaf) {
            let leaf_id = leaf.leaf_hash();
            GovernanceLeavesById::<T>::insert(leaf_id, &leaf);
            EpochGovernanceLeafIds::<T>::mutate(epoch_id, |leaf_ids| {
                if !leaf_ids.contains(&leaf_id) {
                    leaf_ids.push(leaf_id);
                }
            });
        }

        fn note_known_ack_domain(
            proposal_id: GovernanceProposalId,
            acknowledging_domain: DomainId,
            current_epoch: EpochId,
        ) {
            ActivationRecordsById::<T>::mutate(proposal_id, |maybe_record| {
                if let Some(record) = maybe_record {
                    if !record.known_ack_domains.contains(&acknowledging_domain) {
                        record.known_ack_domains.push(acknowledging_domain);
                        record.known_ack_domains.sort();
                        record.known_ack_domains.dedup();
                    }
                }
            });
            Self::maybe_schedule_activation(proposal_id, current_epoch);
        }

        fn maybe_schedule_activation(proposal_id: GovernanceProposalId, current_epoch: EpochId) {
            let Some(mut proposal) = ProposalsById::<T>::get(proposal_id) else {
                return;
            };
            let Some(mut activation) = ActivationRecordsById::<T>::get(proposal_id) else {
                return;
            };
            if !matches!(
                proposal.status,
                GovernanceProposalStatus::LocallyFinalized | GovernanceProposalStatus::Scheduled
            ) {
                return;
            }
            if !Self::has_full_ack_set(&activation.target_domains, &activation.known_ack_domains) {
                return;
            }

            if proposal.status != GovernanceProposalStatus::Scheduled {
                proposal.status = GovernanceProposalStatus::Scheduled;
                activation.status = GovernanceProposalStatus::Scheduled;
                activation.scheduled_at_epoch = Some(current_epoch);
                ProposalsById::<T>::insert(proposal_id, &proposal);
                ActivationRecordsById::<T>::insert(proposal_id, &activation);

                if current_epoch >= activation.activation_epoch {
                    ReadyActivationIds::<T>::mutate(|proposal_ids| {
                        if !proposal_ids.contains(&proposal_id) {
                            proposal_ids.push(proposal_id);
                        }
                    });
                } else {
                    ScheduledProposalIdsByEpoch::<T>::mutate(
                        activation.activation_epoch,
                        |proposal_ids| {
                            if !proposal_ids.contains(&proposal_id) {
                                proposal_ids.push(proposal_id);
                            }
                        },
                    );
                }

                Self::deposit_event(Event::ProposalScheduled {
                    proposal_id,
                    activation_epoch: activation.activation_epoch,
                });
            }
        }

        fn has_full_ack_set(target_domains: &[DomainId], known_ack_domains: &[DomainId]) -> bool {
            target_domains
                .iter()
                .all(|target_domain| known_ack_domains.contains(target_domain))
        }

        fn activate_epoch_bucket(current_epoch: EpochId, block_height: u32) {
            let proposal_ids = ScheduledProposalIdsByEpoch::<T>::get(current_epoch);
            for proposal_id in proposal_ids {
                Self::try_activate_proposal(proposal_id, current_epoch, block_height);
            }
        }

        fn activate_ready_queue(current_epoch: EpochId, block_height: u32) {
            let proposal_ids = ReadyActivationIds::<T>::get();
            let mut remaining = Vec::new();
            for proposal_id in proposal_ids {
                if !Self::try_activate_proposal(proposal_id, current_epoch, block_height) {
                    remaining.push(proposal_id);
                }
            }
            ReadyActivationIds::<T>::put(remaining);
        }

        fn try_activate_proposal(
            proposal_id: GovernanceProposalId,
            current_epoch: EpochId,
            block_height: u32,
        ) -> bool {
            let Some(mut proposal) = ProposalsById::<T>::get(proposal_id) else {
                return true;
            };
            let Some(mut activation) = ActivationRecordsById::<T>::get(proposal_id) else {
                return true;
            };
            if proposal.status != GovernanceProposalStatus::Scheduled {
                return proposal.status == GovernanceProposalStatus::Activated;
            }
            if current_epoch < activation.activation_epoch {
                return false;
            }
            if !Self::has_full_ack_set(&activation.target_domains, &activation.known_ack_domains) {
                return false;
            }

            let new_protocol_version = proposal.payload.protocol_version();
            if ProtocolVersion::<T>::get() != new_protocol_version {
                ProtocolVersion::<T>::put(new_protocol_version);
            }

            proposal.status = GovernanceProposalStatus::Activated;
            activation.status = GovernanceProposalStatus::Activated;
            activation.activated_at_epoch = Some(current_epoch);
            activation.activated_at_local_block_height = Some(block_height);
            ProposalsById::<T>::insert(proposal_id, &proposal);
            ActivationRecordsById::<T>::insert(proposal_id, &activation);

            Self::deposit_event(Event::ProposalActivated {
                proposal_id,
                activation_epoch: activation.activation_epoch,
                new_protocol_version,
            });
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::Encode;
    use frame_support::{
        assert_err, assert_ok, construct_runtime, derive_impl,
        traits::Hooks,
    };
    use ialp_common_types::{
        fixed_bytes, governance_merkle_empty_root, governance_merkle_root,
        governance_proposal_id, ChainIdentity, DomainId, EpochId, GovernanceAckLeaf,
        GovernanceAckLeafHashInput, GovernancePayload, GovernanceProposalLeaf,
        GovernanceProposalLeafHashInput, GovernanceProposalStatus, GovernanceVoteChoice,
        GOVERNANCE_ACK_LEAF_VERSION, GOVERNANCE_ACK_RECORD_VERSION,
        GOVERNANCE_PROPOSAL_LEAF_VERSION, GOVERNANCE_PROPOSAL_VERSION,
    };
    use pallet_balances::AccountData;
    use sp_core::H256;
    use sp_runtime::{
        traits::{BlakeTwo256, IdentityLookup},
        BuildStorage,
    };

    type Block = frame_system::mocking::MockBlock<Test>;

    construct_runtime!(
        pub enum Test {
            System: frame_system,
            Balances: pallet_balances,
            Domain: pallet_ialp_domain,
            Epochs: pallet_ialp_epochs,
            Governance: pallet,
        }
    );

    #[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
    impl frame_system::Config for Test {
        type Block = Block;
        type AccountId = u64;
        type Lookup = IdentityLookup<Self::AccountId>;
        type Hash = H256;
        type Hashing = BlakeTwo256;
        type AccountData = AccountData<u128>;
    }

    impl pallet_balances::Config for Test {
        type MaxLocks = frame_support::traits::ConstU32<50>;
        type MaxReserves = ();
        type ReserveIdentifier = [u8; 8];
        type Balance = u128;
        type RuntimeEvent = RuntimeEvent;
        type DustRemoval = ();
        type ExistentialDeposit = frame_support::traits::ConstU128<1>;
        type AccountStore = System;
        type WeightInfo = ();
        type FreezeIdentifier = RuntimeFreezeReason;
        type MaxFreezes = frame_support::traits::ConstU32<8>;
        type RuntimeHoldReason = RuntimeHoldReason;
        type RuntimeFreezeReason = RuntimeFreezeReason;
        type DoneSlashHandler = ();
    }

    impl pallet_ialp_domain::Config for Test {
        type RuntimeEvent = RuntimeEvent;
    }

    pub struct TestSummaryContext;
    pub struct TestExportCommitmentProvider;
    pub struct TestImportCommitmentProvider;
    pub struct TestGovernanceCommitmentProvider;
    pub struct TestDomainIdentity;
    pub struct TestEpochInfo;

    impl pallet_ialp_epochs::SummaryContext<H256> for TestSummaryContext {
        fn domain_id() -> DomainId {
            Domain::domain_id()
        }

        fn validator_set_hash() -> [u8; 32] {
            [9u8; 32]
        }

        fn hash_to_bytes(hash: &H256) -> [u8; 32] {
            hash.to_fixed_bytes()
        }
    }

    impl pallet_ialp_epochs::ExportCommitmentProvider for TestExportCommitmentProvider {
        fn commit_epoch_exports(_: EpochId, _: u32, _: u32) -> [u8; 32] {
            [0u8; 32]
        }
    }

    impl pallet_ialp_epochs::ImportCommitmentProvider for TestImportCommitmentProvider {
        fn commit_epoch_imports(_: EpochId, _: u32, _: u32) -> [u8; 32] {
            [0u8; 32]
        }
    }

    impl pallet_ialp_epochs::GovernanceCommitmentProvider for TestGovernanceCommitmentProvider {
        fn commit_epoch_governance(
            epoch_id: EpochId,
            start_block_height: u32,
            end_block_height: u32,
        ) -> [u8; 32] {
            Governance::commit_epoch_governance(epoch_id, start_block_height, end_block_height)
        }
    }

    impl DomainIdentityProvider for TestDomainIdentity {
        fn domain_id() -> DomainId {
            Domain::domain_id()
        }
    }

    impl EpochInfoProvider for TestEpochInfo {
        fn current_epoch() -> EpochId {
            Epochs::current_epoch()
        }
    }

    impl pallet_ialp_epochs::Config for Test {
        type RuntimeEvent = RuntimeEvent;
        type SummaryContext = TestSummaryContext;
        type ExportCommitmentProvider = TestExportCommitmentProvider;
        type ImportCommitmentProvider = TestImportCommitmentProvider;
        type GovernanceCommitmentProvider = TestGovernanceCommitmentProvider;
    }

    impl Config for Test {
        type RuntimeEvent = RuntimeEvent;
        type Currency = Balances;
        type DomainIdentity = TestDomainIdentity;
        type EpochInfo = TestEpochInfo;
    }

    fn new_test_ext(domain_id: DomainId) -> sp_io::TestExternalities {
        let mut storage = frame_system::GenesisConfig::<Test>::default()
            .build_storage()
            .expect("frame storage");
        pallet_balances::GenesisConfig::<Test> {
            balances: vec![(1, 100), (2, 100), (3, 100), (4, 100), (99, 100)],
            dev_accounts: None,
        }
        .assimilate_storage(&mut storage)
        .expect("balances storage");
        pallet_ialp_domain::GenesisConfig::<Test> {
            chain_identity: ChainIdentity {
                domain_id,
                chain_id: fixed_bytes(b"ialp-test"),
                chain_name: fixed_bytes(b"IALP Test"),
                token_symbol: fixed_bytes(b"IALP"),
                token_decimals: 12,
            },
            _marker: Default::default(),
        }
        .assimilate_storage(&mut storage)
        .expect("domain storage");
        pallet_ialp_epochs::GenesisConfig::<Test> {
            epoch_length_blocks: 3,
            _marker: Default::default(),
        }
        .assimilate_storage(&mut storage)
        .expect("epochs storage");
        pallet::GenesisConfig::<Test> {
            protocol_version: 1,
            importer_account: Some(99),
            governance_voters: vec![1, 2, 3, 4],
            _marker: Default::default(),
        }
        .assimilate_storage(&mut storage)
        .expect("governance storage");
        storage.into()
    }

    fn execute_block<F>(block_number: u64, parent_hash: H256, action: F) -> H256
    where
        F: FnOnce(),
    {
        System::initialize(&block_number, &parent_hash, &Default::default());
        Domain::on_initialize(block_number);
        Epochs::on_initialize(block_number);
        Governance::on_initialize(block_number);
        action();
        Epochs::on_finalize(block_number);
        System::finalize().hash()
    }

    fn run_to_block(target: u64) {
        let mut parent_hash = H256::zero();
        for block_number in 1..=target {
            parent_hash = execute_block(block_number, parent_hash, || {});
        }
    }

    #[test]
    fn proposal_creation_and_voting_snapshot_are_real() {
        new_test_ext(DomainId::Earth).execute_with(|| {
            let _ = execute_block(1, H256::zero(), || {
                assert_ok!(Governance::create_proposal(
                    RuntimeOrigin::signed(1),
                    vec![DomainId::Moon],
                    2,
                ));
            });

            let proposal_id = governance_proposal_id(DomainId::Earth, 0);
            let proposal = Governance::proposal(proposal_id).expect("proposal exists");
            assert_eq!(proposal.status, GovernanceProposalStatus::Voting);
            assert_eq!(proposal.snapshot_total_voting_power, 400);
            assert_eq!(
                ProposalVotingPowerByVoter::<Test>::get((proposal_id, fixed_bytes(&1u64.encode()))),
                Some(100)
            );
        });
    }

    #[test]
    fn closing_proposal_without_quorum_rejects_it() {
        new_test_ext(DomainId::Earth).execute_with(|| {
            let mut parent_hash = H256::zero();
            parent_hash = execute_block(1, parent_hash, || {
                assert_ok!(Governance::create_proposal(
                    RuntimeOrigin::signed(1),
                    vec![DomainId::Moon],
                    2,
                ));
                let proposal_id = governance_proposal_id(DomainId::Earth, 0);
                assert_ok!(Governance::cast_vote(
                    RuntimeOrigin::signed(1),
                    proposal_id,
                    GovernanceVoteChoice::Yes,
                ));
            });
            parent_hash = execute_block(2, parent_hash, || {});
            parent_hash = execute_block(3, parent_hash, || {});
            let _ = execute_block(4, parent_hash, || {
                let proposal_id = governance_proposal_id(DomainId::Earth, 0);
                assert_ok!(Governance::close_proposal(
                    RuntimeOrigin::signed(2),
                    proposal_id,
                ));
            });

            let proposal =
                Governance::proposal(governance_proposal_id(DomainId::Earth, 0)).expect("proposal");
            assert_eq!(proposal.status, GovernanceProposalStatus::Rejected);
        });
    }

    #[test]
    fn finalized_proposal_creates_real_governance_root() {
        new_test_ext(DomainId::Earth).execute_with(|| {
            let mut parent_hash = H256::zero();
            parent_hash = execute_block(1, parent_hash, || {
                assert_ok!(Governance::create_proposal(
                    RuntimeOrigin::signed(1),
                    vec![DomainId::Moon],
                    2,
                ));
                let proposal_id = governance_proposal_id(DomainId::Earth, 0);
                assert_ok!(Governance::cast_vote(
                    RuntimeOrigin::signed(1),
                    proposal_id,
                    GovernanceVoteChoice::Yes,
                ));
                assert_ok!(Governance::cast_vote(
                    RuntimeOrigin::signed(2),
                    proposal_id,
                    GovernanceVoteChoice::Yes,
                ));
            });
            parent_hash = execute_block(2, parent_hash, || {});
            parent_hash = execute_block(3, parent_hash, || {});
            parent_hash = execute_block(4, parent_hash, || {
                let proposal_id = governance_proposal_id(DomainId::Earth, 0);
                assert_ok!(Governance::close_proposal(
                    RuntimeOrigin::signed(2),
                    proposal_id,
                ));
            });
            parent_hash = execute_block(5, parent_hash, || {});
            parent_hash = execute_block(6, parent_hash, || {});
            let _ = execute_block(7, parent_hash, || {});

            let leaves = Governance::canonical_epoch_governance_leaves(1);
            let header = Epochs::summary_header(1).expect("summary header");
            assert_eq!(
                header.governance_root,
                governance_merkle_root(DomainId::Earth, 1, 4, 6, &leaves)
            );
            assert_ne!(
                header.governance_root,
                governance_merkle_empty_root(DomainId::Earth, 1, 4, 6)
            );
        });
    }

    #[test]
    fn imported_single_target_proposal_schedules_and_activates_once() {
        new_test_ext(DomainId::Moon).execute_with(|| {
            run_to_block(1);
            let leaf = GovernanceProposalLeaf::from_hash_input(GovernanceProposalLeafHashInput {
                version: GOVERNANCE_PROPOSAL_LEAF_VERSION,
                proposal_id: governance_proposal_id(DomainId::Earth, 0),
                source_domain: DomainId::Earth,
                target_domain: DomainId::Moon,
                target_domains: vec![DomainId::Moon],
                proposer: [1u8; 32],
                payload_hash: GovernancePayload::SetProtocolVersion { new_version: 2 }.payload_hash(),
                new_protocol_version: 2,
                created_epoch: 0,
                voting_start_epoch: 0,
                voting_end_epoch: 1,
                approval_epoch: 1,
                activation_epoch: 4,
            });

            assert_ok!(Governance::import_verified_governance_proposal(
                RuntimeOrigin::signed(99),
                ialp_common_types::ImportedGovernanceProposalClaim {
                    version: GOVERNANCE_PROPOSAL_VERSION,
                    leaf,
                    summary_hash: [1u8; 32],
                    package_hash: [2u8; 32],
                },
            ));

            let proposal =
                Governance::proposal(governance_proposal_id(DomainId::Earth, 0)).expect("proposal");
            assert_eq!(proposal.status, GovernanceProposalStatus::Scheduled);
            assert_eq!(Governance::protocol_version(), 1);

            run_to_block(13);

            let proposal =
                Governance::proposal(governance_proposal_id(DomainId::Earth, 0)).expect("proposal");
            let activation = Governance::activation_record(governance_proposal_id(DomainId::Earth, 0))
                .expect("activation");
            assert_eq!(proposal.status, GovernanceProposalStatus::Activated);
            assert_eq!(activation.activated_at_epoch, Some(4));
            assert_eq!(Governance::protocol_version(), 2);
        });
    }

    #[test]
    fn imported_ack_before_known_proposal_is_rejected_on_chain() {
        new_test_ext(DomainId::Earth).execute_with(|| {
            run_to_block(1);
            let ack_leaf = GovernanceAckLeaf::from_hash_input(GovernanceAckLeafHashInput {
                version: GOVERNANCE_ACK_LEAF_VERSION,
                proposal_id: governance_proposal_id(DomainId::Earth, 0),
                source_domain: DomainId::Earth,
                target_domain: DomainId::Earth,
                acknowledging_domain: DomainId::Moon,
                target_domains: vec![DomainId::Moon],
                payload_hash: GovernancePayload::SetProtocolVersion { new_version: 2 }.payload_hash(),
                new_protocol_version: 2,
                activation_epoch: 5,
                acknowledged_epoch: 1,
            });

            assert_err!(
                Governance::import_verified_governance_ack(
                    RuntimeOrigin::signed(99),
                    ialp_common_types::ImportedGovernanceAckClaim {
                        version: GOVERNANCE_ACK_RECORD_VERSION,
                        leaf: ack_leaf,
                        summary_hash: [1u8; 32],
                        package_hash: [2u8; 32],
                    },
                ),
                Error::<Test>::ProposalNotKnown
            );
            assert!(
                Governance::ack_record(
                    (governance_proposal_id(DomainId::Earth, 0), DomainId::Moon)
                )
                .is_none()
            );
        });
    }
}
