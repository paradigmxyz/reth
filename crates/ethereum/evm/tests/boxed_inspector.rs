//! Regression test: a boxed trait-object inspector survives `Evm::transact_with_inspector`.

use alloy_primitives::{Address, TxKind};
use reth_evm::{ConfigureEvm, Evm};
use reth_evm_ethereum::{EthEvmConfig, EthEvmEnv};

#[derive(Debug)]
struct Marker {
    tag: u64,
    // Non-pointer bytes: reading these as a fat pointer would crash or corrupt memory.
    pad: [u8; 64],
}

impl<T: evm2::EvmTypesHost> evm2::Inspector<T> for Marker {}

fn run<E: Evm>(evm: &mut E, tx: &alloy_consensus::transaction::Recovered<E::Transaction>) {
    let boxed: Box<dyn evm2::Inspector<E::EvmTypes>> =
        Box::new(Marker { tag: 0x1234, pad: [0xAA; 64] });
    let (back, _result) = evm.transact_with_inspector(tx, boxed).unwrap();
    let erased: &dyn evm2::evm::NonStaticAny = &*back;
    let marker = erased.downcast_ref::<Marker>().expect("boxed inspector keeps its concrete type");
    assert_eq!(marker.tag, 0x1234);
    assert_eq!(marker.pad, [0xAA; 64]);
}

#[test]
fn transact_with_inspector_returns_boxed_dyn_inspector() {
    let config = EthEvmConfig::mainnet();
    let mut evm = config.evm_with_env(
        evm2::evm::CacheDB::<evm2::evm::EmptyDB>::default(),
        EthEvmEnv::new(evm2::SpecId::CANCUN, Default::default(), 1),
    );
    let tx = alloy_consensus::transaction::Recovered::new_unchecked(
        evm2::ethereum::TxEnvelope::Legacy(alloy_consensus::TxLegacy {
            gas_limit: 1_000_000,
            to: TxKind::Call(Address::with_last_byte(1)),
            ..Default::default()
        }),
        Address::ZERO,
    );
    run(&mut evm, &tx);
}
