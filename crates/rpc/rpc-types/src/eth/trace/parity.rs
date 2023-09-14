#![allow(missing_docs)]
//! Types for trace module.
//!
//! See <https://openethereum.github.io/JSONRPC-trace-module>

use reth_primitives::{Address, Bytes, H256, U256, U64};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
};

/// Different Trace diagnostic targets.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TraceType {
    /// Default trace
    Trace,
    /// Provides a full trace of the VM’s state throughout the execution of the transaction,
    /// including for any subcalls.
    VmTrace,
    /// Provides information detailing all altered portions of the Ethereum state made due to the
    /// execution of the transaction.
    StateDiff,
}

/// The Outcome of a traced transaction with optional settings
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceResults {
    /// Output of the trace
    pub output: Bytes,
    /// Enabled if [TraceType::StateDiff] is provided
    pub state_diff: Option<StateDiff>,
    /// Enabled if [TraceType::Trace] is provided, otherwise an empty vec
    #[serde(default)]
    pub trace: Vec<TransactionTrace>,
    /// Enabled if [TraceType::VmTrace] is provided
    pub vm_trace: Option<VmTrace>,
}

// === impl TraceResults ===

impl TraceResults {
    /// Sets the gas used of the root trace.
    ///
    /// The root trace's gasUsed should mirror the actual gas used by the transaction.
    ///
    /// This allows setting it manually by consuming the execution result's gas for example.
    pub fn set_root_trace_gas_used(&mut self, gas_used: u64) {
        if let Some(r) = self.trace.first_mut().and_then(|t| t.result.as_mut()) {
            r.set_gas_used(gas_used)
        }
    }
}

/// A `FullTrace` with an additional transaction hash
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceResultsWithTransactionHash {
    #[serde(flatten)]
    pub full_trace: TraceResults,
    pub transaction_hash: H256,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChangedType<T> {
    pub from: T,
    pub to: T,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Delta<T> {
    #[default]
    #[serde(rename = "=")]
    Unchanged,
    #[serde(rename = "+")]
    Added(T),
    #[serde(rename = "-")]
    Removed(T),
    #[serde(rename = "*")]
    Changed(ChangedType<T>),
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDiff {
    pub balance: Delta<U256>,
    pub code: Delta<Bytes>,
    pub nonce: Delta<U64>,
    pub storage: BTreeMap<H256, Delta<H256>>,
}

/// New-type for list of account diffs
#[derive(Clone, Debug, Eq, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StateDiff(pub BTreeMap<Address, AccountDiff>);

impl Deref for StateDiff {
    type Target = BTreeMap<Address, AccountDiff>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for StateDiff {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "action")]
pub enum Action {
    Call(CallAction),
    Create(CreateAction),
    /// Parity style traces never renamed suicide to selfdestruct: <https://eips.ethereum.org/EIPS/eip-6>
    ///
    /// For compatibility reasons, this is serialized as `suicide`: <https://github.com/paradigmxyz/reth/issues/3721>
    #[serde(rename = "suicide", alias = "selfdestruct")]
    Selfdestruct(SelfdestructAction),
    Reward(RewardAction),
}

impl Action {
    /// Returns true if this is a call action
    pub fn is_call(&self) -> bool {
        matches!(self, Action::Call(_))
    }

    /// Returns true if this is a create action
    pub fn is_create(&self) -> bool {
        matches!(self, Action::Call(_))
    }

    /// Returns true if this is a selfdestruct action
    pub fn is_selfdestruct(&self) -> bool {
        matches!(self, Action::Selfdestruct(_))
    }
    /// Returns true if this is a reward action
    pub fn is_reward(&self) -> bool {
        matches!(self, Action::Reward(_))
    }
}

/// An external action type.
///
/// Used as enum identifier for [Action]
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    /// Contract call.
    Call,
    /// Contract creation.
    Create,
    /// Contract suicide/selfdestruct.
    #[serde(rename = "suicide", alias = "selfdestruct")]
    Selfdestruct,
    /// A block reward.
    Reward,
}

/// Call type.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallType {
    /// None
    #[default]
    None,
    /// Call
    Call,
    /// Call code
    CallCode,
    /// Delegate call
    DelegateCall,
    /// Static call
    StaticCall,
}

/// Represents a certain [CallType] of a _call_ or message transaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallAction {
    /// Address of the sending account.
    pub from: Address,
    /// The type of the call.
    pub call_type: CallType,
    /// The gas available for executing the call.
    pub gas: U64,
    /// The input data provided to the call.
    pub input: Bytes,
    /// Address of the destination/target account.
    pub to: Address,
    /// Value transferred to the destination account.
    pub value: U256,
}

/// Represents a _create_ action, either a `CREATE` operation or a CREATE transaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAction {
    /// The address of the creator.
    pub from: Address,
    /// The value with which the new account is endowed.
    pub value: U256,
    /// The gas available for the creation init code.
    pub gas: U64,
    /// The init code.
    pub init: Bytes,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RewardType {
    Block,
    Uncle,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardAction {
    /// Author's address.
    pub author: Address,
    /// Reward amount.
    pub value: U256,
    /// Reward type.
    pub reward_type: RewardType,
}

/// Represents a _selfdestruct_ action fka `suicide`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfdestructAction {
    /// destroyed/suicided address.
    pub address: Address,
    /// destroyed contract heir.
    pub refund_address: Address,
    /// Balance of the contract just before it was destroyed.
    pub balance: U256,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOutput {
    pub gas_used: U64,
    pub output: Bytes,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOutput {
    pub gas_used: U64,
    pub code: Bytes,
    pub address: Address,
}

/// Represents the output of a trace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TraceOutput {
    /// Output of a regular call transaction.
    Call(CallOutput),
    /// Output of a CREATE transaction.
    Create(CreateOutput),
}

// === impl TraceOutput ===

impl TraceOutput {
    /// Returns the gas used by this trace.
    pub fn gas_used(&self) -> U64 {
        match self {
            TraceOutput::Call(call) => call.gas_used,
            TraceOutput::Create(create) => create.gas_used,
        }
    }

    /// Sets the gas used by this trace.
    pub fn set_gas_used(&mut self, gas_used: u64) {
        match self {
            TraceOutput::Call(call) => call.gas_used = U64::from(gas_used),
            TraceOutput::Create(create) => create.gas_used = U64::from(gas_used),
        }
    }
}

/// A parity style trace of a transaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionTrace {
    #[serde(flatten)]
    pub action: Action,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub result: Option<TraceOutput>,
    pub subtraces: usize,
    pub trace_address: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalizedTransactionTrace {
    #[serde(flatten)]
    pub trace: TransactionTrace,
    /// Hash of the block, if not pending
    ///
    /// Note: this deviates from <https://openethereum.github.io/JSONRPC-trace-module#trace_transaction> which always returns a block number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<H256>,
    /// Block number the transaction is included in, None if pending.
    ///
    /// Note: this deviates from <https://openethereum.github.io/JSONRPC-trace-module#trace_transaction> which always returns a block number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    /// Hash of the transaction
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_hash: Option<H256>,
    /// Transaction index within the block, None if pending.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_position: Option<u64>,
}

/// A record of a full VM trace for a CALL/CREATE.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmTrace {
    /// The code to be executed.
    pub code: Bytes,
    /// All executed instructions.
    pub ops: Vec<VmInstruction>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmInstruction {
    /// The gas cost for this instruction.
    pub cost: u64,
    /// Information concerning the execution of the operation.
    pub ex: Option<VmExecutedOperation>,
    /// The program counter.
    pub pc: usize,
    /// Subordinate trace of the CALL/CREATE if applicable.
    pub sub: Option<VmTrace>,
    /// Stringified opcode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idx: Option<String>,
}

/// A record of an executed VM operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmExecutedOperation {
    /// The total gas used.
    pub used: u64,
    /// The stack item placed, if any.
    pub push: Vec<U256>,
    /// If altered, the memory delta.
    pub mem: Option<MemoryDelta>,
    /// The altered storage value, if any.
    pub store: Option<StorageDelta>,
}

/// A diff of some chunk of memory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDelta {
    /// Offset into memory the change begins.
    pub off: usize,
    /// The changed data.
    pub data: Bytes,
}

/// A diff of some storage value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageDelta {
    pub key: U256,
    pub val: U256,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_trace() {
        let s = r#"{
            "action": {
                "from": "0x66e29f0b6b1b07071f2fde4345d512386cb66f5f",
                "callType": "call",
                "gas": "0x10bfc",
                "input": "0xf6cd1e8d0000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000011c37937e080000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ec6952892271c8ee13f12e118484e03149281c9600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000010480862479000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000002000000000000000000000000160f5f00288e9e1cc8655b327e081566e580a71d00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000011c37937e080000fffffffffffffffffffffffffffffffffffffffffffffffffee3c86c81f8000000000000000000000000000000000000000000000000000000000000",
                "to": "0x160f5f00288e9e1cc8655b327e081566e580a71d",
                "value": "0x244b"
            },
            "error": "Reverted",
            "result": {
                "gasUsed": "0x9daf",
                "output": "0x000000000000000000000000000000000000000000000000011c37937e080000"
            },
            "subtraces": 3,
            "traceAddress": [],
            "type": "call"
        }"#;
        let val = serde_json::from_str::<TransactionTrace>(s).unwrap();
        serde_json::to_value(val).unwrap();
    }

    #[test]
    fn test_selfdestruct_suicide() {
        let input = r#"{
            "action": {
                "address": "0x66e29f0b6b1b07071f2fde4345d512386cb66f5f",
                "refundAddress": "0x66e29f0b6b1b07071f2fde4345d512386cb66f5f",
                "balance": "0x244b"
            },
            "error": "Reverted",
            "result": {
                "gasUsed": "0x9daf",
                "output": "0x000000000000000000000000000000000000000000000000011c37937e080000"
            },
            "subtraces": 3,
            "traceAddress": [],
            "type": "suicide"
        }"#;
        let val = serde_json::from_str::<TransactionTrace>(input).unwrap();
        assert!(val.action.is_selfdestruct());

        let json = serde_json::to_value(val.clone()).unwrap();
        let expect = serde_json::from_str::<serde_json::Value>(input).unwrap();
        similar_asserts::assert_eq!(json, expect);
        let s = serde_json::to_string(&val).unwrap();
        let json = serde_json::from_str::<serde_json::Value>(&s).unwrap();
        similar_asserts::assert_eq!(json, expect);

        let input = input.replace("suicide", "selfdestruct");
        let val = serde_json::from_str::<TransactionTrace>(&input).unwrap();
        assert!(val.action.is_selfdestruct());
    }
}
