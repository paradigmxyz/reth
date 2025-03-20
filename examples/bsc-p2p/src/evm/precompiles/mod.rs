use super::spec::BscSpecId;
use revm::{
    context::Cfg,
    context_interface::ContextTr,
    handler::{EthPrecompiles, PrecompileProvider},
    interpreter::InterpreterResult,
    precompile::{PrecompileError, Precompiles},
    primitives::{Address, Bytes},
};
use std::boxed::Box;

mod iavl;
mod tendermint;

// Optimism precompile provider
#[derive(Debug, Clone)]
pub struct BscPrecompiles {
    /// Inner precompile provider is same as Ethereums.
    inner: EthPrecompiles,
}

impl BscPrecompiles {
    /// Create a new [`OpPrecompiles`] with the given precompiles.
    pub fn new(precompiles: &'static Precompiles) -> Self {
        Self { inner: EthPrecompiles { precompiles } }
    }

    /// Create a new precompile provider with the given optimismispec.
    #[inline]
    pub fn new_with_spec(spec: BscSpecId) -> Self {
        todo!()
    }

    /// Returns precompiles for Istanbul spec.
    pub fn istanbul(&self) -> &'static Self {
        todo!()
    }
}

impl<CTX> PrecompileProvider<CTX> for BscPrecompiles
where
    CTX: ContextTr<Cfg: Cfg<Spec = BscSpecId>>,
{
    type Output = InterpreterResult;

    #[inline]
    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) {
        *self = Self::new_with_spec(spec);
    }

    #[inline]
    fn run(
        &mut self,
        context: &mut CTX,
        address: &Address,
        bytes: &Bytes,
        gas_limit: u64,
    ) -> Result<Option<Self::Output>, PrecompileError> {
        self.inner.run(context, address, bytes, gas_limit)
    }

    #[inline]
    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        self.inner.warm_addresses()
    }

    #[inline]
    fn contains(&self, address: &Address) -> bool {
        self.inner.contains(address)
    }
}

impl Default for BscPrecompiles {
    fn default() -> Self {
        Self::new_with_spec(BscSpecId::CANCUN)
    }
}
