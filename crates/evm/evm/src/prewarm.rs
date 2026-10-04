//! Worker-owned execution for transaction prewarming.

use crate::{EvmErrorFor, HaltReasonFor, TxEnvFor};
use alloc::boxed::Box;
use alloy_evm::{precompiles::PrecompilesMap, Evm, EvmError};
use revm::context::result::{HaltReasonTr, ResultAndState};

/// Execution resources retained by one transaction-prewarming worker for one block.
///
/// This deliberately exposes neither database ownership nor canonical commit operations.
/// Returned changes are proof-prefetch hints only. Implementations must reset transaction-local
/// state after every result or error and must not commit speculative writes into the parent.
/// The runner is created and destroyed on its owning thread, so no `Send` bound is required.
pub trait PrewarmRunner {
    /// Transaction environment consumed by this runner.
    type Tx;
    /// Execution or provider error returned to the existing prewarming error path.
    type Error: EvmError;
    /// Chain-specific execution halt reason.
    type HaltReason: HaltReasonTr;

    /// Executes one transaction without committing its returned state.
    fn transact(&mut self, tx: Self::Tx) -> Result<ResultAndState<Self::HaltReason>, Self::Error>;

    /// Exposes the precompile set for the existing pure-precompile cache decoration.
    ///
    /// Any execution mode selected by an implementation must honor mutations performed through
    /// this method. It must not bypass customized precompiles using an independent cached EVM.
    fn precompiles_mut(&mut self) -> &mut PrecompilesMap;
}

/// An owned, thread-local prewarming runner with a configuration's exact transaction and errors.
pub type BoxedPrewarmRunner<C, DB> = Box<
    dyn PrewarmRunner<
        Tx = TxEnvFor<C>,
        Error = EvmErrorFor<C, <DB as revm::Database>::Error>,
        HaltReason = HaltReasonFor<C>,
    >,
>;

/// Default adapter preserving an EVM's transaction and precompile access behavior.
///
/// An explicit adapter avoids introducing another `transact` method on all EVM implementations.
#[derive(Debug)]
pub struct EvmPrewarmRunner<E> {
    evm: E,
}

impl<E> EvmPrewarmRunner<E> {
    /// Wraps an existing EVM without changing its database or execution configuration.
    pub const fn new(evm: E) -> Self {
        Self { evm }
    }
}

impl<E: Evm<Precompiles = PrecompilesMap>> PrewarmRunner for EvmPrewarmRunner<E> {
    type Tx = E::Tx;
    type Error = E::Error;
    type HaltReason = E::HaltReason;

    fn transact(&mut self, tx: Self::Tx) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.evm.transact(tx)
    }

    fn precompiles_mut(&mut self) -> &mut PrecompilesMap {
        self.evm.precompiles_mut()
    }
}
