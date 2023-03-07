use crate::tracing::{arena::LogCallOrder, call::CallTrace};
use reth_primitives::{bytes::Bytes, H256};

/// Ethereum log.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct RawLog {
    /// Indexed event params are represented as log topics.
    pub(crate) topics: Vec<H256>,
    /// Others are just plain data.
    pub(crate) data: Bytes,
}

/// A node in the arena
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallTraceNode {
    /// Parent node index in the arena
    pub(crate) parent: Option<usize>,
    /// Children node indexes in the arena
    pub(crate) children: Vec<usize>,
    /// This node's index in the arena
    pub(crate) idx: usize,
    /// The call trace
    pub(crate) trace: CallTrace,
    /// Logs
    pub(crate) logs: Vec<RawLog>,
    /// Ordering of child calls and logs
    pub(crate) ordering: Vec<LogCallOrder>,
}

impl CallTraceNode {
    // /// Returns the kind of call the trace belongs to
    // pub fn kind(&self) -> CallKind {
    //     self.trace.kind
    // }
    //
    // /// Returns the status of the call
    // pub fn status(&self) -> Return {
    //     self.trace.status
    // }

    // /// Returns the `Res` for a parity trace
    // pub fn parity_result(&self) -> Res {
    //     match self.kind() {
    //         CallKind::Call | CallKind::StaticCall | CallKind::CallCode | CallKind::DelegateCall
    // => {             Res::Call(CallResult {
    //                 gas_used: self.trace.gas_cost.into(),
    //                 output: self.trace.output.to_raw().into(),
    //             })
    //         }
    //         CallKind::Create | CallKind::Create2 => Res::Create(CreateResult {
    //             gas_used: self.trace.gas_cost.into(),
    //             code: self.trace.output.to_raw().into(),
    //             address: self.trace.address,
    //         }),
    //     }
    // }
    //
    // /// Returns the `Action` for a parity trace
    // pub fn parity_action(&self) -> Action {
    //     if self.status() == Return::SelfDestruct {
    //         return Action::Suicide(Suicide {
    //             address: self.trace.address,
    //             // TODO deserialize from calldata here?
    //             refund_address: Default::default(),
    //             balance: self.trace.value,
    //         })
    //     }
    //     match self.kind() {
    //         CallKind::Call | CallKind::StaticCall | CallKind::CallCode | CallKind::DelegateCall
    // => {             Action::Call(Call {
    //                 from: self.trace.caller,
    //                 to: self.trace.address,
    //                 value: self.trace.value,
    //                 gas: self.trace.gas_cost.into(),
    //                 input: self.trace.data.to_raw().into(),
    //                 call_type: self.kind().into(),
    //             })
    //         }
    //         CallKind::Create | CallKind::Create2 => Action::Create(Create {
    //             from: self.trace.caller,
    //             value: self.trace.value,
    //             gas: self.trace.gas_cost.into(),
    //             init: self.trace.data.to_raw().into(),
    //         }),
    //     }
    // }
}
