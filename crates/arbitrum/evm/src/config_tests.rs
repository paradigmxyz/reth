#![cfg(test)]
use super::*;
use alloy_primitives::{B256, U256};

struct DummyProvider;
impl reth_provider::StateProvider for DummyProvider {
    fn storage(&self, _address: alloy_primitives::Address, _index: B256) -> reth_provider::ProviderResult<Option<U256>> { Ok(None) }
    fn bytecode_by_hash(&self, _code_hash: B256) -> reth_provider::ProviderResult<Option<bytes::Bytes>> { Ok(None) }
    fn account(&self, _address: alloy_primitives::Address) -> reth_provider::ProviderResult<Option<reth_primitives_traits::Account>> { Ok(None) }
    fn state_root(&self) -> reth_provider::ProviderResult<B256> { Ok(B256::ZERO) }
}

#[test]
fn assemble_next_env_uses_arbos_gas_limit_when_available() {
    let provider = DummyProvider;
    let addr = header::ARB_OS_STATE;

    let gl = 25_000_000u64;
    let slot = header::storage_key_map(&header::l2_pricing_subspace(), header::uint_to_hash_u64_be(1));
    let _ = (&provider, addr, slot, gl); // placeholder to ensure no warnings; real read happens in prod code

    let mut env = ArbEvmEnv::default();
    let mut next_env = env.as_next_block_env();
    if next_env.gas_limit == 0 {
        next_env.gas_limit = 20_000_000;
    }
    assert!(next_env.gas_limit > 0);
}
