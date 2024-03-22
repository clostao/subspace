use crate::{self as pallet_auto_id};
use frame_support::traits::{ConstU16, ConstU32, ConstU64};
use sp_core::H256;
use sp_runtime::traits::{BlakeTwo256, IdentityLookup};
use sp_runtime::BuildStorage;
use subspace_runtime_primitives::Moment;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub struct Test {
        System: frame_system,
        Timestamp: pallet_timestamp,
        AutoId: pallet_auto_id,
    }
);

impl pallet_auto_id::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Time = Timestamp;
}

const SLOT_DURATION: u64 = 1000;

impl pallet_timestamp::Config for Test {
    /// A timestamp: milliseconds since the unix epoch.
    type Moment = Moment;
    type OnTimestampSet = ();
    type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
    type WeightInfo = ();
}

impl frame_system::Config for Test {
    type BaseCallFilter = frame_support::traits::Everything;
    type BlockWeights = ();
    type BlockLength = ();
    type DbWeight = ();
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    type RuntimeTask = RuntimeTask;
    type Nonce = u64;
    type Hash = H256;
    type Hashing = BlakeTwo256;
    type AccountId = u64;
    type Lookup = IdentityLookup<Self::AccountId>;
    type Block = Block;
    type RuntimeEvent = RuntimeEvent;
    type BlockHashCount = ConstU64<250>;
    type Version = ();
    type PalletInfo = PalletInfo;
    type AccountData = ();
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type SystemWeightInfo = ();
    type SS58Prefix = ConstU16<42>;
    type OnSetCode = ();
    type MaxConsumers = ConstU32<16>;
}

#[allow(dead_code)]
pub(crate) fn new_test_ext() -> sp_io::TestExternalities {
    let t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    t.into()
}
