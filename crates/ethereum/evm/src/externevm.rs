//! ExternEVM custom EvmFactory.
//!
//! Wraps the standard `EthEvmFactory` and injects the API_CALL precompile
//! at address 0x00000000000000000000000000000000000000AA.
//!
//! Milestone 1: The precompile ignores all input and returns ABI-encoded uint256(1234).

use alloy_evm::{
    eth::EthEvmContext,
    evm::EvmFactory,
    precompiles::{DynPrecompile, PrecompileInput, PrecompilesMap},
    Database, Evm, EvmEnv,
};
use alloy_primitives::Address;
use revm::{
    context::{BlockEnv, TxEnv},
    context_interface::result::{EVMError, HaltReason},
    inspector::NoOpInspector,
    precompile::{PrecompileHalt, PrecompileId, PrecompileOutput, PrecompileResult},
    primitives::hardfork::SpecId,
    Inspector,
};

/// The address of the API_CALL precompile: 0x00000000000000000000000000000000000000AA
pub const API_CALL_ADDRESS: Address = {
    let mut addr = [0u8; 20];
    addr[19] = 0xAA;
    Address::new(addr)
};

/// Fixed gas cost for API_CALL precompile.
const API_CALL_GAS: u64 = 3_000;

/// The precompile ID for API_CALL.
fn api_call_id() -> PrecompileId {
    PrecompileId::Custom("API_CALL".into())
}

/// API_CALL precompile function.
/// Milestone 1: ignores all input, returns ABI-encoded uint256(1234).
fn api_call_precompile(input: PrecompileInput<'_>) -> PrecompileResult {
    if input.gas < API_CALL_GAS {
        return Ok(PrecompileOutput::halt(PrecompileHalt::OutOfGas, input.reservoir));
    }

    // ABI-encode uint256(1234): 32 bytes, big-endian
    // 1234 = 0x04D2
    let mut output = [0u8; 32];
    output[30] = 0x04;
    output[31] = 0xD2;

    Ok(PrecompileOutput::new(API_CALL_GAS, output.to_vec().into(), input.reservoir))
}

/// Creates a `DynPrecompile` for the API_CALL precompile.
fn api_call_dyn_precompile() -> DynPrecompile {
    DynPrecompile::new_stateful(api_call_id(), api_call_precompile)
}

/// Injects the API_CALL precompile into a `PrecompilesMap`.
pub fn inject_api_call_precompile(precompiles: &mut PrecompilesMap) {
    precompiles.apply_precompile(&API_CALL_ADDRESS, |_| Some(api_call_dyn_precompile()));
}

/// Custom EVM factory that wraps `EthEvmFactory` and adds the API_CALL precompile.
#[derive(Debug, Clone, Default)]
pub struct ExternEvmFactory {
    inner: alloy_evm::EthEvmFactory,
}

impl ExternEvmFactory {
    /// Creates a new `ExternEvmFactory`.
    pub fn new() -> Self {
        Self::default()
    }
}

impl EvmFactory for ExternEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>>> =
        <alloy_evm::EthEvmFactory as EvmFactory>::Evm<DB, I>;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv,
    ) -> Self::Evm<DB, NoOpInspector> {
        let mut evm = self.inner.create_evm(db, input);
        inject_api_call_precompile(evm.precompiles_mut());
        evm
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let mut evm = self.inner.create_evm_with_inspector(db, input, inspector);
        inject_api_call_precompile(evm.precompiles_mut());
        evm
    }
}
