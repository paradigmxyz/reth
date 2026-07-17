//! Custom transaction envelope and evm2 registry handler.

use crate::config::{
    CustomMessageExt, CustomMessageResultExt, CustomTxEnvExt, CustomTxResultExt, CustomTypes,
};
use alloy_eips::eip2718::Typed2718;
use alloy_primitives::{Address, Bytes};
use evm2::{
    bytecode::Bytecode,
    env::TxEnv,
    interpreter::{Host, Message},
    registry::{HandlerResult, TxRegistry, TxRequest},
};

/// Type byte for the example execute-code transaction.
pub const EXECUTE_CODE_TX_TYPE: u8 = 0x7f;

/// Minimal custom transaction envelope used by the evm2 registry path.
#[derive(Debug)]
pub enum CustomEnvelope {
    /// Execute the supplied bytecode as a message.
    ExecuteCode(ExecuteCodeTx),
}

impl Typed2718 for CustomEnvelope {
    fn ty(&self) -> u8 {
        match self {
            Self::ExecuteCode(tx) => tx.ty(),
        }
    }
}

impl CustomEnvelope {
    /// Returns the execute-code transaction, if this is that variant.
    pub const fn as_execute_code(&self) -> Option<&ExecuteCodeTx> {
        match self {
            Self::ExecuteCode(tx) => Some(tx),
        }
    }
}

/// Payload for [`CustomEnvelope::ExecuteCode`].
#[derive(Debug)]
pub struct ExecuteCodeTx {
    /// Recovered sender used as the message caller.
    pub caller: Address,
    /// Message destination.
    pub target: Address,
    /// Code executed by the message.
    pub code: Bytes,
    /// Message gas limit.
    pub gas_limit: u64,
}

impl ExecuteCodeTx {
    /// Returns the EIP-2718 type byte.
    pub const fn ty(&self) -> u8 {
        EXECUTE_CODE_TX_TYPE
    }
}

/// Handles the custom execute-code transaction.
pub fn execute_code(
    req: TxRequest<'_, '_, CustomTypes, ExecuteCodeTx>,
) -> HandlerResult<evm2::TxResult<CustomTypes>> {
    // The transaction handler owns policy; the interpreter still executes a normal message.
    let mut message = Message {
        gas_limit: req.tx.gas_limit,
        destination: req.tx.target,
        code_address: req.tx.target,
        caller: req.tx.caller,
        ext: CustomMessageExt { is_system: false },
        ..Message::default()
    };
    let tx_env = TxEnv { ext: CustomTxEnvExt { label: "execute-code" }, ..TxEnv::default() };
    let mut result =
        req.host.execute_message(&tx_env, Bytecode::new_legacy(req.tx.code.clone()), &mut message);
    result.ext = CustomMessageResultExt { handled_custom_message: true };
    Ok(evm2::TxResult::<CustomTypes> {
        status: result.stop.is_success(),
        total_gas_spent: req.tx.gas_limit - result.gas.remaining(),
        stop: result.stop,
        output: result.output,
        ext: CustomTxResultExt { handled_custom_tx: result.ext.handled_custom_message },
        ..Default::default()
    })
}

/// Creates the registry for the custom transaction type.
pub fn custom_registry() -> TxRegistry<CustomTypes, evm2::TxResult<CustomTypes>> {
    TxRegistry::new().with_handler(
        EXECUTE_CODE_TX_TYPE,
        CustomEnvelope::as_execute_code,
        execute_code,
    )
}
