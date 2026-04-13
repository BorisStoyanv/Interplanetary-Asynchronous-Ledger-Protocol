#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use frame_support::{
        pallet_prelude::*,
        traits::fungible::{InspectHold, MutateHold},
    };
    use frame_system::pallet_prelude::*;
    use ialp_common_types::{
        export_id, export_merkle_root, fixed_bytes, AccountIdBytes, DomainId, EpochId, ExportId,
        ExportLeaf, ExportLeafHashInput, ExportRecord, ExportStatus, ImportObservationStatus,
        ObservedImportClaim, ObservedImportRecord,
    };
    use sp_runtime::traits::SaturatedConversion;
    use sp_std::vec::Vec;

    pub trait DomainIdentityProvider {
        fn domain_id() -> DomainId;
    }

    pub trait EpochInfoProvider {
        fn current_epoch() -> EpochId;
    }

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: InspectHold<Self::AccountId, Balance = u128, Reason = Self::RuntimeHoldReason>
            + MutateHold<Self::AccountId, Balance = u128, Reason = Self::RuntimeHoldReason>;
        type RuntimeHoldReason: From<HoldReason>;
        type DomainIdentity: DomainIdentityProvider;
        type EpochInfo: EpochInfoProvider;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::composite_enum]
    pub enum HoldReason {
        #[codec(index = 0)]
        CrossDomainTransfer,
    }

    #[pallet::storage]
    #[pallet::getter(fn next_export_sequence)]
    pub type NextExportSequence<T> = StorageValue<_, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn epoch_export_ids)]
    #[pallet::unbounded]
    pub type EpochExportIds<T> =
        StorageMap<_, Blake2_128Concat, EpochId, Vec<ExportId>, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn export_record)]
    #[pallet::unbounded]
    pub type ExportsById<T> = StorageMap<_, Blake2_128Concat, ExportId, ExportRecord, OptionQuery>;

    #[pallet::storage]
    #[pallet::getter(fn importer_account)]
    pub type ImporterAccount<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;

    #[pallet::storage]
    #[pallet::getter(fn observed_import)]
    #[pallet::unbounded]
    pub type ObservedImportsById<T> =
        StorageMap<_, Blake2_128Concat, ExportId, ObservedImportRecord, OptionQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub importer_account: Option<T::AccountId>,
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                importer_account: None,
                _marker: Default::default(),
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            if let Some(account) = &self.importer_account {
                ImporterAccount::<T>::put(account);
            }
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        CrossDomainTransferCreated {
            export_id: ExportId,
            epoch_id: EpochId,
            target_domain: DomainId,
            amount: u128,
        },
        ExportMarkedCommitted {
            export_id: ExportId,
            epoch_id: EpochId,
        },
        ImportObserved {
            export_id: ExportId,
            source_domain: DomainId,
            target_domain: DomainId,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        SameDomainTarget,
        ZeroAmount,
        HoldFailed,
        MissingImporterAccount,
        UnauthorizedImporter,
        WrongTargetDomain,
        DuplicateObservedImport,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::call_index(0)]
        #[pallet::weight(10_000)]
        pub fn create_cross_domain_transfer(
            origin: OriginFor<T>,
            target_domain: DomainId,
            recipient: AccountIdBytes,
            amount: u128,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(amount > 0, Error::<T>::ZeroAmount);

            let source_domain = T::DomainIdentity::domain_id();
            ensure!(target_domain != source_domain, Error::<T>::SameDomainTarget);

            T::Currency::hold(&Self::hold_reason(), &who, amount)
                .map_err(|_| Error::<T>::HoldFailed)?;

            let sequence = NextExportSequence::<T>::get();
            NextExportSequence::<T>::put(sequence.saturating_add(1));

            let current_epoch = T::EpochInfo::current_epoch();
            let current_block_height: u32 =
                frame_system::Pallet::<T>::block_number().saturated_into();
            let extrinsic_index = frame_system::Pallet::<T>::extrinsic_index().unwrap_or_default();
            let export_id = export_id(source_domain, sequence);
            let leaf = ExportLeaf::from_hash_input(ExportLeafHashInput {
                version: ialp_common_types::EXPORT_LEAF_VERSION,
                export_id,
                source_domain,
                target_domain,
                sender: Self::account_to_bytes(&who),
                recipient,
                amount,
                source_epoch_id: current_epoch,
                source_block_height: current_block_height,
                extrinsic_index,
            });

            ExportsById::<T>::insert(
                export_id,
                ExportRecord {
                    leaf,
                    status: ExportStatus::LocalFinal,
                },
            );
            EpochExportIds::<T>::mutate(current_epoch, |export_ids: &mut Vec<ExportId>| {
                export_ids.push(export_id)
            });

            Self::deposit_event(Event::CrossDomainTransferCreated {
                export_id,
                epoch_id: current_epoch,
                target_domain,
                amount,
            });

            Ok(())
        }

        #[pallet::call_index(1)]
        #[pallet::weight(10_000)]
        pub fn observe_verified_import(
            origin: OriginFor<T>,
            claim: ObservedImportClaim,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let configured_importer =
                ImporterAccount::<T>::get().ok_or(Error::<T>::MissingImporterAccount)?;
            ensure!(who == configured_importer, Error::<T>::UnauthorizedImporter);
            ensure!(
                claim.target_domain == T::DomainIdentity::domain_id(),
                Error::<T>::WrongTargetDomain
            );
            ensure!(
                !ObservedImportsById::<T>::contains_key(claim.export_id),
                Error::<T>::DuplicateObservedImport
            );

            let observed_at_local_block_height: u32 =
                frame_system::Pallet::<T>::block_number().saturated_into();
            let record = ObservedImportRecord {
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
                observer_account: Self::account_to_bytes(&who),
                status: ImportObservationStatus::RemoteObserved,
            };

            ObservedImportsById::<T>::insert(claim.export_id, record);
            Self::deposit_event(Event::ImportObserved {
                export_id: claim.export_id,
                source_domain: claim.source_domain,
                target_domain: claim.target_domain,
            });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn canonical_epoch_exports(epoch_id: EpochId) -> Vec<ExportLeaf> {
            let mut leaves = EpochExportIds::<T>::get(epoch_id)
                .into_iter()
                .filter_map(|export_id| ExportsById::<T>::get(export_id).map(|record| record.leaf))
                .collect::<Vec<_>>();
            ialp_common_types::sort_export_leaves(&mut leaves);
            leaves
        }

        pub fn commit_epoch_exports(
            epoch_id: EpochId,
            start_block_height: u32,
            end_block_height: u32,
        ) -> [u8; 32] {
            let domain_id = T::DomainIdentity::domain_id();
            let leaves = Self::canonical_epoch_exports(epoch_id);
            let export_root = export_merkle_root(
                domain_id,
                epoch_id,
                start_block_height,
                end_block_height,
                &leaves,
            );

            for leaf in &leaves {
                ExportsById::<T>::mutate(leaf.export_id, |maybe_record| {
                    if let Some(record) = maybe_record {
                        if record.status != ExportStatus::Exported {
                            record.status = ExportStatus::Exported;
                            Self::deposit_event(Event::ExportMarkedCommitted {
                                export_id: leaf.export_id,
                                epoch_id,
                            });
                        }
                    }
                });
            }

            export_root
        }

        pub fn account_to_bytes(account: &T::AccountId) -> AccountIdBytes {
            fixed_bytes(&account.encode())
        }

        fn hold_reason() -> T::RuntimeHoldReason {
            HoldReason::CrossDomainTransfer.into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_support::{
        assert_noop, assert_ok, construct_runtime, derive_impl,
        traits::{fungible::InspectHold, Hooks},
    };
    use ialp_common_types::{
        build_export_inclusion_proof, export_merkle_empty_root, export_merkle_root, fixed_bytes,
        summary_header_storage_key, ChainIdentity, DomainId, EpochId, ExportStatus,
        ImportObservationStatus, ObservedImportClaim,
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
            Transfers: pallet,
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
        fn commit_epoch_exports(
            epoch_id: EpochId,
            start_block_height: u32,
            end_block_height: u32,
        ) -> [u8; 32] {
            Transfers::commit_epoch_exports(epoch_id, start_block_height, end_block_height)
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
    }

    impl Config for Test {
        type RuntimeEvent = RuntimeEvent;
        type Currency = Balances;
        type RuntimeHoldReason = RuntimeHoldReason;
        type DomainIdentity = TestDomainIdentity;
        type EpochInfo = TestEpochInfo;
    }

    fn new_test_ext() -> sp_io::TestExternalities {
        let mut storage = frame_system::GenesisConfig::<Test>::default()
            .build_storage()
            .expect("frame storage");
        pallet_balances::GenesisConfig::<Test> {
            balances: vec![(1, 1_000), (2, 1_000), (3, 1_000), (99, 1_000)],
            dev_accounts: None,
        }
        .assimilate_storage(&mut storage)
        .expect("balances storage");
        pallet_ialp_domain::GenesisConfig::<Test> {
            chain_identity: ChainIdentity {
                domain_id: DomainId::Earth,
                chain_id: fixed_bytes(b"ialp-earth-local"),
                chain_name: fixed_bytes(b"IALP Earth"),
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
            importer_account: Some(99),
            _marker: Default::default(),
        }
        .assimilate_storage(&mut storage)
        .expect("transfers storage");
        storage.into()
    }

    fn execute_block<F>(block_number: u64, parent_hash: H256, action: F) -> H256
    where
        F: FnOnce(),
    {
        System::initialize(&block_number, &parent_hash, &Default::default());
        Domain::on_initialize(block_number);
        Epochs::on_initialize(block_number);
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
    fn creating_transfer_records_export_and_holds_balance() {
        new_test_ext().execute_with(|| {
            let _ = execute_block(1, H256::zero(), || {
                System::set_extrinsic_index(0);
                assert_ok!(Transfers::create_cross_domain_transfer(
                    RuntimeOrigin::signed(1),
                    DomainId::Moon,
                    fixed_bytes(b"moon-recipient"),
                    75,
                ));
            });

            let export_id = EpochExportIds::<Test>::get(0)[0];
            let record = Transfers::export_record(export_id).expect("record exists");

            assert_eq!(record.status, ExportStatus::LocalFinal);
            assert_eq!(record.leaf.source_epoch_id, 0);
            assert_eq!(record.leaf.source_block_height, 1);
            assert_eq!(record.leaf.target_domain, DomainId::Moon);
            assert_eq!(
                Balances::balance_on_hold(
                    &RuntimeHoldReason::Transfers(HoldReason::CrossDomainTransfer),
                    &1,
                ),
                75
            );
        });
    }

    #[test]
    fn same_domain_targets_are_rejected() {
        new_test_ext().execute_with(|| {
            let _ = execute_block(1, H256::zero(), || {
                System::set_extrinsic_index(0);
                assert_noop!(
                    Transfers::create_cross_domain_transfer(
                        RuntimeOrigin::signed(1),
                        DomainId::Earth,
                        fixed_bytes(b"earth-recipient"),
                        10,
                    ),
                    Error::<Test>::SameDomainTarget
                );
            });
        });
    }

    #[test]
    fn zero_amount_transfers_are_rejected() {
        new_test_ext().execute_with(|| {
            let _ = execute_block(1, H256::zero(), || {
                System::set_extrinsic_index(0);
                assert_noop!(
                    Transfers::create_cross_domain_transfer(
                        RuntimeOrigin::signed(1),
                        DomainId::Moon,
                        fixed_bytes(b"moon-recipient"),
                        0,
                    ),
                    Error::<Test>::ZeroAmount
                );
            });
        });
    }

    #[test]
    fn export_marked_committed_only_on_epoch_commit() {
        new_test_ext().execute_with(|| {
            let mut parent_hash = H256::zero();
            parent_hash = execute_block(1, parent_hash, || {
                System::set_extrinsic_index(0);
                assert_ok!(Transfers::create_cross_domain_transfer(
                    RuntimeOrigin::signed(1),
                    DomainId::Moon,
                    fixed_bytes(b"moon-recipient"),
                    50,
                ));
            });

            let created_events = System::events();
            assert!(created_events.iter().any(|event| matches!(
                event.event,
                RuntimeEvent::Transfers(Event::CrossDomainTransferCreated { .. })
            )));
            assert!(!created_events.iter().any(|event| matches!(
                event.event,
                RuntimeEvent::Transfers(Event::ExportMarkedCommitted { .. })
            )));

            parent_hash = execute_block(2, parent_hash, || {});
            parent_hash = execute_block(3, parent_hash, || {});
            let _ = execute_block(4, parent_hash, || {});

            assert!(System::events().iter().any(|event| matches!(
                event.event,
                RuntimeEvent::Transfers(Event::ExportMarkedCommitted { epoch_id: 0, .. })
            )));
        });
    }

    #[test]
    fn export_root_is_real_and_canonical_order_ignores_storage_order() {
        new_test_ext().execute_with(|| {
            let mut parent_hash = H256::zero();
            parent_hash = execute_block(1, parent_hash, || {
                System::set_extrinsic_index(0);
                assert_ok!(Transfers::create_cross_domain_transfer(
                    RuntimeOrigin::signed(1),
                    DomainId::Moon,
                    fixed_bytes(b"moon-a"),
                    11,
                ));
                System::set_extrinsic_index(1);
                assert_ok!(Transfers::create_cross_domain_transfer(
                    RuntimeOrigin::signed(2),
                    DomainId::Mars,
                    fixed_bytes(b"mars-a"),
                    22,
                ));
            });
            parent_hash = execute_block(2, parent_hash, || {});
            parent_hash = execute_block(3, parent_hash, || {});

            let mut shuffled = EpochExportIds::<Test>::get(0);
            shuffled.reverse();
            EpochExportIds::<Test>::insert(0, shuffled);

            let leaves = Transfers::canonical_epoch_exports(0);
            let expected_root = export_merkle_root(DomainId::Earth, 0, 1, 3, &leaves);

            let _ = execute_block(4, parent_hash, || {});

            let header = Epochs::summary_header(0).expect("summary header exists");
            assert_eq!(header.export_root, expected_root);
            assert_ne!(
                header.export_root,
                export_merkle_empty_root(DomainId::Earth, 0, 1, 3)
            );
        });
    }

    #[test]
    fn observe_verified_import_requires_allowlisted_importer_and_prevents_duplicates() {
        new_test_ext().execute_with(|| {
            run_to_block(1);
            let claim = ObservedImportClaim {
                version: ialp_common_types::OBSERVED_IMPORT_VERSION,
                export_id: [7u8; 32],
                source_domain: DomainId::Moon,
                target_domain: DomainId::Earth,
                source_epoch_id: 4,
                summary_hash: [1u8; 32],
                package_hash: [2u8; 32],
                recipient: fixed_bytes(b"recipient"),
                amount: 99,
            };

            assert_eq!(
                Transfers::observe_verified_import(RuntimeOrigin::signed(1), claim.clone()),
                Err(Error::<Test>::UnauthorizedImporter.into())
            );
            assert!(Transfers::observed_import(claim.export_id).is_none());

            assert_ok!(Transfers::observe_verified_import(
                RuntimeOrigin::signed(99),
                claim.clone(),
            ));

            let stored =
                Transfers::observed_import(claim.export_id).expect("observed import stored");
            assert_eq!(stored.status, ImportObservationStatus::RemoteObserved);

            assert_eq!(
                Transfers::observe_verified_import(RuntimeOrigin::signed(99), claim.clone()),
                Err(Error::<Test>::DuplicateObservedImport.into())
            );
            assert!(Transfers::observed_import(claim.export_id).is_some());
        });
    }

    #[test]
    fn export_proof_builds_against_real_summary_root() {
        new_test_ext().execute_with(|| {
            let mut parent_hash = H256::zero();
            parent_hash = execute_block(1, parent_hash, || {
                System::set_extrinsic_index(0);
                assert_ok!(Transfers::create_cross_domain_transfer(
                    RuntimeOrigin::signed(1),
                    DomainId::Moon,
                    fixed_bytes(b"moon-a"),
                    10,
                ));
                System::set_extrinsic_index(1);
                assert_ok!(Transfers::create_cross_domain_transfer(
                    RuntimeOrigin::signed(1),
                    DomainId::Moon,
                    fixed_bytes(b"moon-b"),
                    20,
                ));
            });
            parent_hash = execute_block(2, parent_hash, || {});
            parent_hash = execute_block(3, parent_hash, || {});
            let _ = execute_block(4, parent_hash, || {});

            let leaves = Transfers::canonical_epoch_exports(0);
            let export_id = leaves[0].export_id;
            let proof = build_export_inclusion_proof(&leaves, export_id).expect("proof exists");
            let header = Epochs::summary_header(0).expect("summary header exists");

            assert!(ialp_common_types::verify_export_inclusion_proof(
                header.export_root,
                &proof
            ));
            assert_ne!(
                summary_header_storage_key(0),
                ialp_common_types::epoch_export_ids_storage_key(0)
            );
        });
    }
}
