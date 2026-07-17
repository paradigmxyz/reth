//! This example shows how to configure an evm2-backed custom EVM.
//!
//! It exercises the three extension points exposed by the integration:
//!
//! - a custom precompile provider,
//! - a custom opcode table, and
//! - a custom transaction registry and transaction envelope.
//!
//! The node path uses ordinary Ethereum wire transactions with the custom opcode and precompile.
//! The standalone path below additionally uses a custom evm2 transaction type. A production node
//! that puts that transaction on the wire must pair the same factory with custom node primitives,
//! pool, network, and payload components.

#![allow(missing_docs, clippy::missing_const_for_fn)]
#![warn(unused_crate_dependencies)]

use alloy_eips::eip2718::Typed2718;
use alloy_genesis::Genesis;
use alloy_primitives::{Address, Bytes, U256};
use config::{CustomBlockEnvExt, CustomSpecId, CustomTypes};
use evm2::{
    env::BlockEnv,
    evm::{precompile::NoPrecompiles, AccountInfo, InMemoryDB},
    interpreter::{op, InstrStop},
    registry::HandlerResult,
    Evm,
};
use factory::{custom_block_env, CustomEvmFactory, NodeEvmFactory, CUSTOM_PRECOMPILE_ADDRESS};
use reth_ethereum::{
    chainspec::{Chain, ChainSpec},
    evm::{EthEvmConfig, EvmFactory},
    node::{
        api::{FullNodeTypes, NodeTypes},
        builder::{components::ExecutorBuilder, BuilderContext, NodeBuilder},
        core::{args::RpcServerArgs, node_config::NodeConfig},
        node::EthereumAddOns,
        EthereumNode,
    },
    tasks::Runtime,
    EthPrimitives,
};
use reth_tracing::{RethTracer, Tracer};

mod config;
mod factory;
mod opcode;
mod tx;
mod wire;

use tx::{CustomEnvelope, ExecuteCodeTx};
use wire::{into_evm_transaction, recover_wire_transaction, SignedCustomTransaction, WireTx};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    custom_opcode()?;
    l1_blocknumber_opcode()?;
    custom_precompile()?;
    custom_transaction()?;
    custom_wire_transaction()?;
    mainnet_fallback()?;
    launch_node().await
}

fn custom_opcode() -> HandlerResult<()> {
    let mut evm = custom_evm();
    let tx = custom_opcode_tx(Bytes::from_static(&[
        opcode::CUSTOM_OPCODE,
        op::PUSH1,
        0x01,
        op::SSTORE,
        op::STOP,
    ]));

    let result = evm.transact(&tx)?.commit();
    let expected_gas = u64::from(opcode::CUSTOM_OPCODE_GAS) +
        u64::from(opcode::CUSTOM_OPCODE_DYNAMIC_GAS) +
        3 +
        2100 +
        20_000;

    assert_eq!(result.stop, InstrStop::Stop);
    assert!(result.status);
    assert_eq!(result.tx_gas_used(), expected_gas);
    assert!(result.ext.handled_custom_tx);
    println!(
        "custom opcode: type=0x{:02x} status={} gas_used={}",
        tx.ty(),
        result.status,
        result.tx_gas_used()
    );
    Ok(())
}

fn custom_precompile() -> HandlerResult<()> {
    let target = Address::from([0xcc; 20]);
    let mut code =
        vec![op::PUSH1, 0, op::PUSH1, 0, op::PUSH1, 0, op::PUSH1, 0, op::PUSH1, 0, op::PUSH20];
    code.extend_from_slice(CUSTOM_PRECOMPILE_ADDRESS.as_slice());
    code.extend_from_slice(&[op::PUSH2, 0xff, 0xff, op::CALL, op::STOP]);

    let factory = CustomEvmFactory;
    let spec = CustomSpecId::CustomOsaka;
    let mut evm = Evm::<CustomTypes>::new_with_execution_config(
        factory.execution_config(spec, factory.version(spec.into(), 1)),
        spec,
        BlockEnv { ext: CustomBlockEnvExt { l1_block_number: 42 }, ..custom_block_env() },
        factory.tx_registry(spec),
        InMemoryDB::default(),
        factory.precompiles(spec),
    );
    let result = evm.transact(&CustomEnvelope::ExecuteCode(ExecuteCodeTx {
        caller: Address::ZERO,
        target,
        code: code.into(),
        gas_limit: 100_000,
    }))?;
    let status = result.result().status;
    drop(result);
    assert!(status);
    assert!(evm.precompiles().contains(&CUSTOM_PRECOMPILE_ADDRESS));
    println!("custom precompile: address={CUSTOM_PRECOMPILE_ADDRESS}");
    Ok(())
}

fn l1_blocknumber_opcode() -> HandlerResult<()> {
    let mut evm = custom_evm();
    let tx = custom_opcode_tx(Bytes::from_static(&[
        opcode::L1_BLOCKNUMBER_OPCODE,
        op::PUSH0,
        op::MSTORE,
        op::PUSH1,
        32,
        op::PUSH0,
        op::RETURN,
    ]));

    let result = evm.transact(&tx)?.discard();
    let expected = Bytes::copy_from_slice(&U256::from(42_u64).to_be_bytes::<32>());
    assert_eq!(result.stop, InstrStop::Return);
    assert!(result.status);
    assert_eq!(result.output, expected);
    assert!(result.ext.handled_custom_tx);
    println!("L1 blocknumber opcode: output={expected:?}");
    Ok(())
}

fn custom_transaction() -> HandlerResult<()> {
    let target = Address::from([0xcc; 20]);
    let mut database = InMemoryDB::default();
    database.insert_account_info(&target, AccountInfo::default().with_nonce(1));
    let mut evm = custom_evm_with_database(database);
    let tx = CustomEnvelope::ExecuteCode(ExecuteCodeTx {
        caller: Address::ZERO,
        target,
        gas_limit: 100_000,
        code: Bytes::from_static(&[opcode::CUSTOM_OPCODE, op::PUSH1, 0x01, op::SSTORE, op::STOP]),
    });

    let result = evm.transact(&tx)?.commit();
    assert!(result.status);
    assert!(result.ext.handled_custom_tx);
    println!("custom transaction: type=0x{:02x} handled=true", tx.ty());
    Ok(())
}

fn mainnet_fallback() -> HandlerResult<()> {
    let mut evm = mainnet_evm();
    let tx = custom_opcode_tx(Bytes::from_static(&[opcode::CUSTOM_OPCODE, op::STOP]));

    let result = evm.transact(&tx)?.discard();
    assert_eq!(result.stop, InstrStop::InvalidOpcode);
    assert!(!result.status);
    println!("mainnet fallback: custom opcode remains invalid");
    Ok(())
}

fn custom_wire_transaction() -> HandlerResult<()> {
    let wire = SignedCustomTransaction::new_unhashed(
        WireTx {
            chain_id: 1,
            nonce: 0,
            gas_limit: 100_000,
            max_fee_per_gas: 10,
            max_priority_fee_per_gas: 1,
            target: Address::from([0xcc; 20]),
            code: Bytes::from_static(&[op::STOP]),
        },
        alloy_primitives::Signature::new(U256::from(1), U256::from(2), false),
    );
    let encoded = alloy_eips::Encodable2718::encoded_2718(&wire);
    let decoded = alloy_eips::Decodable2718::decode_2718_exact(&encoded).unwrap();
    let recovered = recover_wire_transaction(decoded).unwrap();
    let caller = recovered.signer();
    let evm_tx = into_evm_transaction(recovered);
    let CustomEnvelope::ExecuteCode(tx) = evm_tx;
    assert_eq!(tx.caller, caller);
    assert_eq!(tx.target, Address::from([0xcc; 20]));
    assert_eq!(tx.code, Bytes::from_static(&[op::STOP]));
    println!("custom wire transaction: type=0x{:02x} roundtrip=true", WireTx::tx_type());
    Ok(())
}

fn custom_opcode_tx(code: Bytes) -> CustomEnvelope {
    CustomEnvelope::ExecuteCode(ExecuteCodeTx {
        caller: Address::ZERO,
        target: Address::from([0xcc; 20]),
        code,
        gas_limit: 100_000,
    })
}

fn custom_evm() -> Evm<'static, CustomTypes> {
    custom_evm_with_database(InMemoryDB::default())
}

fn custom_evm_with_database(database: InMemoryDB) -> Evm<'static, CustomTypes> {
    let factory = CustomEvmFactory;
    let spec = CustomSpecId::CustomOsaka;
    let version = factory.version(spec.into(), 1);
    Evm::<CustomTypes>::new_with_execution_config(
        factory.execution_config(spec, version),
        spec,
        BlockEnv { ext: CustomBlockEnvExt { l1_block_number: 42 }, ..BlockEnv::default() },
        factory.tx_registry(spec),
        database,
        factory.precompiles(spec),
    )
}

fn mainnet_evm() -> Evm<'static, CustomTypes> {
    let factory = CustomEvmFactory;
    let spec = CustomSpecId::MainnetOsaka;
    let version = factory.version(spec.into(), 1);
    Evm::<CustomTypes>::new_with_execution_config(
        factory.execution_config(spec, version),
        spec,
        BlockEnv::default(),
        factory.tx_registry(spec),
        InMemoryDB::default(),
        NoPrecompiles::default(),
    )
}

/// Builds a regular Ethereum executor that uses the custom node factory.
#[derive(Debug, Default, Clone, Copy)]
pub struct MyExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for MyExecutorBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>>,
{
    type EVM = EthEvmConfig<ChainSpec, NodeEvmFactory>;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        Ok(EthEvmConfig::new_with_evm_factory(ctx.chain_spec(), NodeEvmFactory))
    }
}

async fn launch_node() -> eyre::Result<()> {
    let _guard = RethTracer::new().init()?;
    let runtime = Runtime::test();
    let spec = ChainSpec::builder()
        .chain(Chain::mainnet())
        .genesis(Genesis::default())
        .london_activated()
        .paris_activated()
        .shanghai_activated()
        .cancun_activated()
        .prague_activated()
        .osaka_activated()
        .build();
    let node_config =
        NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec);

    let handle = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components().executor(MyExecutorBuilder))
        .with_add_ons(EthereumAddOns::default())
        .launch()
        .await?;
    handle.node_exit_future.await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_extensions_execute() {
        custom_opcode().unwrap();
        l1_blocknumber_opcode().unwrap();
        custom_precompile().unwrap();
        custom_transaction().unwrap();
        custom_wire_transaction().unwrap();
        mainnet_fallback().unwrap();
    }
}
