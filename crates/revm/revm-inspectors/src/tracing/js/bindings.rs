//! Type bindings for js tracing inspector

use reth_primitives::{Address, H256, U256, Bytes};
use revm::interpreter::{OpCode, Stack};
use boa_engine::{class::{Class, ClassBuilder}, error::JsNativeError, native_function::NativeFunction, property::Attribute, Context, JsArgs, JsResult, JsString, JsValue, Source, JsError};
use serde::{Deserialize, Serialize};

/// The Log object that is passed to the javascript inspector.
#[derive(Debug)]
pub struct StepLog {
    /// Stack before step execution
    stack: Stack,
    /// Opcode to be executed
    op: OpCode,
    /// All allocated memory in a step
    memory: Bytes,
    /// Program counter before step execution
    pc: u64,
    /// Remaining gas before step execution
    gas: u64,
    /// Gas cost of step execution
    cost: u64,
    /// Call depth
    depth: usize,
    /// Gas refund counter before step execution
    refund: u64,
    /// The contract object available to the js inspector
    contract: Contract,
}

/// Represents the contract object
#[derive(Debug, Serialize, Deserialize)]
pub struct Contract {
    caller: Address,
    contract: Address,
    value: U256,
    input: Bytes,
}

impl Class for Contract {
    const NAME: &'static str = "contract";

    fn constructor(this: &JsValue, args: &[JsValue], context: &mut Context<'_>) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("unimplemented")
            .into())
    }

    fn init(class: &mut ClassBuilder<'_, '_>) -> JsResult<()> {
        todo!()
    }
}

/// The `ctx` object that represents the context in which the transaction is executed.
pub struct EvmContext {
    // TODO more fields
    block_hash: Option<H256>,
    tx_index: Option<usize>,
    tx_hash: Option<H256>,
}

/// DB is the object that allows the js inspector to interact with the database.
pub struct Db<DB> {
    db: DB,
}
