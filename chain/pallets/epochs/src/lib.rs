#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use codec::{Decode, Encode, MaxEncodedLen};
    use frame_support::{pallet_prelude::*, traits::Get};
    use frame_system::pallet_prelude::*;
    use ialp_common_types::{
        empty_commitment_root, event_envelope_hash, fold_epoch_accumulator, seed_epoch_accumulator,
        tx_envelope_hash, DomainId, EpochId, EpochSummaryHashInput, EpochSummaryHeader,
        StagedSummaryRecord, BLOCK_ROOT_LABEL, EMPTY_HASH, EPOCH_SUMMARY_VERSION, EVENT_ROOT_LABEL,
        EXPORT_ROOT_EMPTY_LABEL, GOVERNANCE_ROOT_EMPTY_LABEL, IMPORT_ROOT_EMPTY_LABEL,
        TX_ROOT_LABEL,
    };
    use scale_info::TypeInfo;
    use sp_runtime::{
        traits::{SaturatedConversion, Zero},
        StateVersion,
    };

    pub trait SummaryContext<Hash> {
        fn domain_id() -> DomainId;
        fn validator_set_hash() -> [u8; 32];
        fn hash_to_bytes(hash: &Hash) -> [u8; 32];
    }

    #[derive(
        Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Default,
    )]
    pub enum SummarySlotStatus {
        #[default]
        Reserved,
        Staged,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Default)]
    pub struct PendingBlockObservation {
        pub block_number: u32,
        pub state_root: [u8; 32],
        pub extrinsic_count: u32,
        pub all_extrinsics_len: u32,
        pub event_count: u32,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Default)]
    pub struct EpochAccumulators {
        pub state_root: [u8; 32],
        pub block_root: [u8; 32],
        pub tx_root: [u8; 32],
        pub event_root: [u8; 32],
        pub blocks_observed: u32,
        pub last_observed_block: u32,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Default)]
    pub struct SummarySlotRecord {
        pub epoch_id: EpochId,
        pub start_height: u32,
        pub end_height: Option<u32>,
        pub reserved_at_block: u32,
        pub staged_at_block_number: Option<u32>,
        pub last_touched_block: u32,
        pub header: Option<EpochSummaryHeader>,
        pub status: SummarySlotStatus,
        pub accumulators: EpochAccumulators,
    }

    /// Phase 1A makes the local chain author the canonical staged summary header at epoch close.
    /// Later transport layers consume these on-chain headers; they do not invent their own.
    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type SummaryContext: SummaryContext<Self::Hash>;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::storage]
    #[pallet::getter(fn epoch_length_blocks)]
    pub type EpochLengthBlocks<T> = StorageValue<_, u32, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn current_epoch)]
    pub type CurrentEpoch<T> = StorageValue<_, EpochId, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn current_epoch_start)]
    pub type CurrentEpochStart<T> = StorageValue<_, u32, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn summary_slot)]
    pub type SummarySlots<T: Config> =
        StorageMap<_, Blake2_128Concat, EpochId, SummarySlotRecord, OptionQuery>;

    #[pallet::storage]
    #[pallet::getter(fn summary_header)]
    pub type SummaryHeaders<T: Config> =
        StorageMap<_, Blake2_128Concat, EpochId, EpochSummaryHeader, OptionQuery>;

    #[pallet::storage]
    #[pallet::getter(fn latest_summary_header)]
    pub type LatestSummaryHeader<T: Config> = StorageValue<_, EpochSummaryHeader, OptionQuery>;

    #[pallet::storage]
    #[pallet::getter(fn latest_summary_hash)]
    pub type LatestSummaryHash<T> = StorageValue<_, [u8; 32], ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn summary_count)]
    pub type SummaryCount<T> = StorageValue<_, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn pending_block_observation)]
    pub type PendingBlockObservationStore<T> =
        StorageValue<_, PendingBlockObservation, OptionQuery>;

    #[pallet::storage]
    #[pallet::getter(fn initialized)]
    pub type Initialized<T> = StorageValue<_, bool, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub epoch_length_blocks: u32,
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                epoch_length_blocks: 300,
                _marker: Default::default(),
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            EpochLengthBlocks::<T>::put(self.epoch_length_blocks.max(1));
            CurrentEpoch::<T>::put(0);
            CurrentEpochStart::<T>::put(1);
            LatestSummaryHash::<T>::put(EMPTY_HASH);
            SummaryCount::<T>::put(0);
            Initialized::<T>::put(false);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        EpochClosed {
            epoch_id: EpochId,
            start_height: u32,
            end_height: u32,
        },
        SummarySlotReserved {
            epoch_id: EpochId,
        },
        EpochSummaryCreated {
            epoch_id: EpochId,
            staged_at_block_number: u32,
            summary_hash: [u8; 32],
        },
        SummaryHashCommitted {
            epoch_id: EpochId,
            summary_hash: [u8; 32],
            prev_summary_hash: [u8; 32],
        },
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(block_number: BlockNumberFor<T>) -> Weight {
            if block_number.is_zero() {
                return Weight::zero();
            }

            let block_number_u32: u32 = block_number.saturated_into();

            if !Initialized::<T>::get() {
                Self::reserve_slot(0, 1, block_number_u32);
                Initialized::<T>::put(true);
            }

            if block_number_u32 > 1 {
                Self::apply_parent_block_observation(block_number_u32);
            }

            Weight::zero()
        }

        fn on_finalize(block_number: BlockNumberFor<T>) {
            if block_number.is_zero() {
                return;
            }

            let block_number_u32: u32 = block_number.saturated_into();
            Self::store_pending_block_observation(block_number_u32);
        }
    }

    impl<T: Config> Pallet<T> {
        fn apply_parent_block_observation(block_number_u32: u32) {
            let parent_block_number = block_number_u32 - 1;
            let Some(observation) = PendingBlockObservationStore::<T>::take() else {
                return;
            };

            debug_assert_eq!(observation.block_number, parent_block_number);

            let current_epoch = CurrentEpoch::<T>::get();
            SummarySlots::<T>::mutate(current_epoch, |maybe_slot| {
                if let Some(slot) = maybe_slot {
                    Self::fold_observation_into_slot(slot, parent_block_number, &observation);
                    slot.last_touched_block = block_number_u32;
                }
            });

            let epoch_length = EpochLengthBlocks::<T>::get().max(1);
            if parent_block_number.is_multiple_of(epoch_length) {
                Self::stage_summary_for_closed_epoch(
                    current_epoch,
                    parent_block_number,
                    block_number_u32,
                );
            }
        }

        fn stage_summary_for_closed_epoch(
            epoch_id: EpochId,
            end_height: u32,
            opening_block_number: u32,
        ) {
            // Phase 1A authors the canonical header in-runtime at the epoch boundary. Later
            // phases add GRANDPA proof packaging around this exact header instead of rebuilding it
            // off-chain.
            let start_height = CurrentEpochStart::<T>::get();
            let prev_summary_hash = if epoch_id == 0 {
                EMPTY_HASH
            } else {
                LatestSummaryHash::<T>::get()
            };
            let domain_id = T::SummaryContext::domain_id();
            let validator_set_hash = T::SummaryContext::validator_set_hash();

            SummarySlots::<T>::mutate(epoch_id, |maybe_slot| {
                let Some(slot) = maybe_slot else {
                    return;
                };

                if slot.status == SummarySlotStatus::Staged {
                    return;
                }

                let header = EpochSummaryHeader::from_hash_input(EpochSummaryHashInput {
                    version: EPOCH_SUMMARY_VERSION,
                    domain_id,
                    epoch_id,
                    prev_summary_hash,
                    start_block_height: start_height,
                    end_block_height: end_height,
                    state_root: slot.accumulators.state_root,
                    block_root: slot.accumulators.block_root,
                    tx_root: slot.accumulators.tx_root,
                    event_root: slot.accumulators.event_root,
                    export_root: empty_commitment_root(
                        EXPORT_ROOT_EMPTY_LABEL,
                        domain_id,
                        epoch_id,
                        start_height,
                        end_height,
                    ),
                    import_root: empty_commitment_root(
                        IMPORT_ROOT_EMPTY_LABEL,
                        domain_id,
                        epoch_id,
                        start_height,
                        end_height,
                    ),
                    governance_root: empty_commitment_root(
                        GOVERNANCE_ROOT_EMPTY_LABEL,
                        domain_id,
                        epoch_id,
                        start_height,
                        end_height,
                    ),
                    validator_set_hash,
                });

                slot.end_height = Some(end_height);
                slot.staged_at_block_number = Some(opening_block_number);
                slot.last_touched_block = opening_block_number;
                slot.header = Some(header.clone());
                slot.status = SummarySlotStatus::Staged;

                // Phase 2A treats `SummaryHeaders[epoch_id]` as the canonical write-once entry
                // proved by storage inclusion. Rewrites would invalidate that proof contract.
                debug_assert!(!SummaryHeaders::<T>::contains_key(epoch_id));
                SummaryHeaders::<T>::insert(epoch_id, &header);
                LatestSummaryHeader::<T>::put(header.clone());
                LatestSummaryHash::<T>::put(header.summary_hash);
                SummaryCount::<T>::mutate(|count| *count += 1);

                Self::deposit_event(Event::EpochClosed {
                    epoch_id,
                    start_height,
                    end_height,
                });
                Self::deposit_event(Event::EpochSummaryCreated {
                    epoch_id,
                    staged_at_block_number: opening_block_number,
                    summary_hash: header.summary_hash,
                });
                Self::deposit_event(Event::SummaryHashCommitted {
                    epoch_id,
                    summary_hash: header.summary_hash,
                    prev_summary_hash,
                });
            });

            let next_epoch = epoch_id + 1;
            CurrentEpoch::<T>::put(next_epoch);
            CurrentEpochStart::<T>::put(opening_block_number);
            Self::reserve_slot(next_epoch, opening_block_number, opening_block_number);
        }

        fn fold_observation_into_slot(
            slot: &mut SummarySlotRecord,
            block_number: u32,
            observation: &PendingBlockObservation,
        ) {
            // The accumulator inputs stay intentionally small and deterministic in Phase 1A.
            // Proof-grade roots for transfers/imports/governance are deferred, but the contract is
            // already fixed now so later phases extend commitments without redefining the header.
            let block_hash =
                T::SummaryContext::hash_to_bytes(&frame_system::Pallet::<T>::parent_hash());
            slot.accumulators.state_root = observation.state_root;
            slot.accumulators.block_root =
                fold_epoch_accumulator(slot.accumulators.block_root, block_number, block_hash);
            slot.accumulators.tx_root = fold_epoch_accumulator(
                slot.accumulators.tx_root,
                block_number,
                tx_envelope_hash(
                    block_number,
                    observation.extrinsic_count,
                    observation.all_extrinsics_len,
                ),
            );
            slot.accumulators.event_root = fold_epoch_accumulator(
                slot.accumulators.event_root,
                block_number,
                event_envelope_hash(block_number, observation.event_count),
            );
            slot.accumulators.blocks_observed = slot.accumulators.blocks_observed.saturating_add(1);
            slot.accumulators.last_observed_block = block_number;
        }

        fn store_pending_block_observation(block_number: u32) {
            let state_root = Self::current_storage_root();
            let observation = PendingBlockObservation {
                block_number,
                state_root,
                extrinsic_count: frame_system::Pallet::<T>::extrinsic_count(),
                all_extrinsics_len: frame_system::Pallet::<T>::all_extrinsics_len(),
                event_count: frame_system::Pallet::<T>::event_count(),
            };
            PendingBlockObservationStore::<T>::put(observation);
        }

        fn current_storage_root() -> [u8; 32] {
            // This captures the Phase 1A canonical runtime state commitment for the block in
            // `on_finalize`. `docs/18_EPOCH_SUMMARY_SPEC.md` locks this semantic explicitly.
            let version: StateVersion = T::Version::get().state_version();
            let storage_root = T::Hash::decode(&mut &sp_io::storage::root(version)[..])
                .expect("runtime hash and storage backend use the same hash type");
            T::SummaryContext::hash_to_bytes(&storage_root)
        }

        fn reserve_slot(epoch_id: EpochId, start_height: u32, block_number: u32) {
            let domain_id = T::SummaryContext::domain_id();
            let record = SummarySlotRecord {
                epoch_id,
                start_height,
                end_height: None,
                reserved_at_block: block_number,
                staged_at_block_number: None,
                last_touched_block: block_number,
                header: None,
                status: SummarySlotStatus::Reserved,
                accumulators: EpochAccumulators {
                    state_root: EMPTY_HASH,
                    block_root: seed_epoch_accumulator(BLOCK_ROOT_LABEL, domain_id, epoch_id),
                    tx_root: seed_epoch_accumulator(TX_ROOT_LABEL, domain_id, epoch_id),
                    event_root: seed_epoch_accumulator(EVENT_ROOT_LABEL, domain_id, epoch_id),
                    blocks_observed: 0,
                    last_observed_block: 0,
                },
            };

            SummarySlots::<T>::insert(epoch_id, record);
            Self::deposit_event(Event::SummarySlotReserved { epoch_id });
        }

        pub fn staged_summary_record(epoch_id: EpochId) -> Option<StagedSummaryRecord> {
            let slot = SummarySlots::<T>::get(epoch_id)?;
            let header = slot.header.clone()?;
            let staged_at_block_number = slot.staged_at_block_number?;

            Some(StagedSummaryRecord {
                header,
                staged_at_block_number,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_support::{construct_runtime, derive_impl, traits::Hooks};
    use ialp_common_types::{
        seed_epoch_accumulator, summary_header_storage_key, DomainId, BLOCK_ROOT_LABEL, EMPTY_HASH,
        EPOCH_SUMMARY_VERSION, EVENT_ROOT_LABEL, TX_ROOT_LABEL,
    };
    use sp_core::H256;
    use sp_runtime::{
        traits::{BlakeTwo256, IdentityLookup},
        BuildStorage,
    };

    type Block = frame_system::mocking::MockBlock<Test>;

    construct_runtime!(
        pub enum Test {
            System: frame_system,
            Epochs: pallet,
        }
    );

    #[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
    impl frame_system::Config for Test {
        type Block = Block;
        type AccountId = u64;
        type Lookup = IdentityLookup<Self::AccountId>;
        type Hash = H256;
        type Hashing = BlakeTwo256;
    }

    pub struct TestSummaryContext;

    impl SummaryContext<H256> for TestSummaryContext {
        fn domain_id() -> DomainId {
            DomainId::Earth
        }

        fn validator_set_hash() -> [u8; 32] {
            [9u8; 32]
        }

        fn hash_to_bytes(hash: &H256) -> [u8; 32] {
            hash.to_fixed_bytes()
        }
    }

    impl Config for Test {
        type RuntimeEvent = RuntimeEvent;
        type SummaryContext = TestSummaryContext;
    }

    fn new_test_ext() -> sp_io::TestExternalities {
        let mut storage = frame_system::GenesisConfig::<Test>::default()
            .build_storage()
            .expect("frame storage");
        pallet::GenesisConfig::<Test> {
            epoch_length_blocks: 3,
            _marker: Default::default(),
        }
        .assimilate_storage(&mut storage)
        .expect("epochs storage");
        storage.into()
    }

    fn execute_block(block_number: u64, parent_hash: H256) -> H256 {
        System::initialize(&block_number, &parent_hash, &Default::default());
        Epochs::on_initialize(block_number);
        Epochs::on_finalize(block_number);
        System::finalize().hash()
    }

    fn run_to_block(target: u64) {
        let mut parent_hash = H256::zero();
        for block_number in 1..=target {
            parent_hash = execute_block(block_number, parent_hash);
        }
    }

    #[test]
    fn reserves_initial_slot_with_seeded_accumulators() {
        new_test_ext().execute_with(|| {
            run_to_block(1);

            let slot = Epochs::summary_slot(0).expect("slot exists");
            assert_eq!(slot.start_height, 1);
            assert_eq!(slot.status, SummarySlotStatus::Reserved);
            assert_eq!(
                slot.accumulators.block_root,
                seed_epoch_accumulator(BLOCK_ROOT_LABEL, DomainId::Earth, 0)
            );
            assert_eq!(
                slot.accumulators.tx_root,
                seed_epoch_accumulator(TX_ROOT_LABEL, DomainId::Earth, 0)
            );
            assert_eq!(
                slot.accumulators.event_root,
                seed_epoch_accumulator(EVENT_ROOT_LABEL, DomainId::Earth, 0)
            );
        });
    }

    #[test]
    fn first_epoch_creates_staged_header_and_zero_prev_hash() {
        new_test_ext().execute_with(|| {
            run_to_block(4);

            let slot = Epochs::summary_slot(0).expect("slot exists");
            let header = Epochs::summary_header(0).expect("summary header exists");

            assert_eq!(slot.status, SummarySlotStatus::Staged);
            assert_eq!(slot.end_height, Some(3));
            assert_eq!(slot.staged_at_block_number, Some(4));
            assert_eq!(header.prev_summary_hash, EMPTY_HASH);
            assert_eq!(header.version, EPOCH_SUMMARY_VERSION);
            assert_eq!(header.start_block_height, 1);
            assert_eq!(header.end_block_height, 3);
            assert_eq!(header.validator_set_hash, [9u8; 32]);
            assert_eq!(header.summary_hash, header.compute_summary_hash());
            assert_eq!(Epochs::latest_summary_hash(), header.summary_hash);
            assert_eq!(
                Epochs::latest_summary_header().expect("latest summary exists"),
                header
            );
            assert_eq!(Epochs::summary_count(), 1);
            assert_eq!(
                Epochs::staged_summary_record(0)
                    .expect("staged summary view exists")
                    .staged_at_block_number,
                4
            );
        });
    }

    #[test]
    fn consecutive_epochs_chain_prev_summary_hash() {
        new_test_ext().execute_with(|| {
            run_to_block(7);

            let first = Epochs::summary_header(0).expect("first header exists");
            let second = Epochs::summary_header(1).expect("second header exists");

            assert_eq!(second.prev_summary_hash, first.summary_hash);
            assert_eq!(Epochs::summary_count(), 2);
            assert_eq!(Epochs::latest_summary_hash(), second.summary_hash);
        });
    }

    #[test]
    fn identical_block_sequences_produce_identical_headers() {
        let first = new_test_ext().execute_with(|| {
            run_to_block(4);
            Epochs::summary_header(0).expect("summary header exists")
        });
        let second = new_test_ext().execute_with(|| {
            run_to_block(4);
            Epochs::summary_header(0).expect("summary header exists")
        });

        assert_eq!(first, second);
    }

    #[test]
    fn summary_creation_happens_once_per_boundary() {
        new_test_ext().execute_with(|| {
            run_to_block(5);

            assert_eq!(Epochs::summary_count(), 1);
            assert!(Epochs::summary_header(1).is_none());

            let current_slot = Epochs::summary_slot(1).expect("next slot exists");
            assert_eq!(current_slot.status, SummarySlotStatus::Reserved);
            assert_eq!(Epochs::current_epoch(), 1);
        });
    }

    #[test]
    fn canonical_summary_header_storage_key_matches_runtime_storage_key() {
        assert_eq!(
            summary_header_storage_key(7),
            SummaryHeaders::<Test>::hashed_key_for(7)
        );
    }

    #[test]
    fn canonical_summary_header_storage_key_differs_from_latest_cache_key() {
        assert_ne!(
            summary_header_storage_key(0),
            LatestSummaryHeader::<Test>::hashed_key().to_vec()
        );
    }

    #[test]
    fn summary_headers_are_immutable_after_stage() {
        new_test_ext().execute_with(|| {
            let mut parent_hash = H256::zero();
            for block_number in 1..=4 {
                parent_hash = execute_block(block_number, parent_hash);
            }
            let staged = Epochs::summary_header(0).expect("summary header exists");
            let staged_count = Epochs::summary_count();

            for block_number in 5..=6 {
                parent_hash = execute_block(block_number, parent_hash);
            }

            assert_eq!(Epochs::summary_header(0), Some(staged));
            assert_eq!(Epochs::summary_count(), staged_count);
        });
    }
}
