#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;
    use ialp_common_types::{fixed_bytes, ChainIdentity, DomainId, CHAIN_ID_BYTES};
    use sp_runtime::traits::Zero;

    /// IALP treats domain identity as protocol state. Exporters, importers, and
    /// aggregators need the same chain-visible value instead of an off-chain config guess.
    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::storage]
    #[pallet::getter(fn chain_identity)]
    pub type ChainIdentityStore<T: Config> = StorageValue<_, ChainIdentity, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn domain_id)]
    pub type ConfiguredDomainId<T: Config> = StorageValue<_, DomainId, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn identity_announced)]
    pub type IdentityAnnounced<T: Config> = StorageValue<_, bool, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub chain_identity: ChainIdentity,
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                chain_identity: ChainIdentity {
                    domain_id: DomainId::Earth,
                    chain_id: fixed_bytes(b"ialp-earth-local"),
                    chain_name: fixed_bytes(b"IALP Earth"),
                    token_symbol: fixed_bytes(b"IALP"),
                    token_decimals: 12,
                },
                _marker: Default::default(),
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            ChainIdentityStore::<T>::put(self.chain_identity.clone());
            ConfiguredDomainId::<T>::put(self.chain_identity.domain_id);
            IdentityAnnounced::<T>::put(false);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        DomainIdentityActivated {
            domain_id: DomainId,
            chain_id: [u8; CHAIN_ID_BYTES],
        },
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(block_number: BlockNumberFor<T>) -> Weight {
            if block_number.is_zero() || IdentityAnnounced::<T>::get() {
                return Weight::zero();
            }

            let identity = ChainIdentityStore::<T>::get();
            Self::deposit_event(Event::DomainIdentityActivated {
                domain_id: identity.domain_id,
                chain_id: identity.chain_id,
            });
            IdentityAnnounced::<T>::put(true);
            Weight::zero()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_support::{construct_runtime, derive_impl, traits::Hooks};
    use ialp_common_types::{fixed_bytes, ChainIdentity, DomainId};
    use sp_core::H256;
    use sp_runtime::{
        traits::{BlakeTwo256, IdentityLookup},
        BuildStorage,
    };

    type Block = frame_system::mocking::MockBlock<Test>;

    construct_runtime!(
        pub enum Test {
            System: frame_system,
            Domain: pallet,
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

    impl Config for Test {
        type RuntimeEvent = RuntimeEvent;
    }

    fn new_test_ext() -> sp_io::TestExternalities {
        let mut storage = frame_system::GenesisConfig::<Test>::default()
            .build_storage()
            .expect("frame storage");
        pallet::GenesisConfig::<Test> {
            chain_identity: ChainIdentity {
                domain_id: DomainId::Mars,
                chain_id: fixed_bytes(b"ialp-mars-local"),
                chain_name: fixed_bytes(b"IALP Mars"),
                token_symbol: fixed_bytes(b"IALP"),
                token_decimals: 12,
            },
            _marker: Default::default(),
        }
        .assimilate_storage(&mut storage)
        .expect("domain storage");

        storage.into()
    }

    #[test]
    fn emits_identity_event_once() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            Domain::on_initialize(1);
            assert_eq!(Domain::domain_id(), DomainId::Mars);
            assert!(Domain::identity_announced());

            let events = System::events();
            assert_eq!(events.len(), 1);
        });
    }
}
