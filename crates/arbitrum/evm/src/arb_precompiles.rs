// Arbitrum predeploys registered as revm precompiles
use alloy_primitives::{Address, Bytes};
use revm::precompile::{Precompile, PrecompileId, PrecompileResult, PrecompileOutput, PrecompileErrors};
use std::sync::OnceLock;

/// Returns Arbitrum precompiles including predeploys
pub fn arbitrum_precompiles() -> &'static revm::precompile::Precompiles {
    static INSTANCE: OnceLock<revm::precompile::Precompiles> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        use revm::handler::EthPrecompiles;
        let mut precompiles = EthPrecompiles::cancun().precompiles.clone();

        // Register Arbitrum predeploys as precompiles
        // These will intercept calls before the EVM tries to load the 0xFE bytecode
        use arb_alloy_predeploys as pre;

        let predeploy_addresses = [
            (Address::from(pre::ARB_SYS), "ArbSys"),
            (Address::from(pre::ARB_RETRYABLE_TX), "ArbRetryableTx"),
            (Address::from(pre::ARB_GAS_INFO), "ArbGasInfo"),
            (Address::from(pre::ARB_ADDRESS_TABLE), "ArbAddressTable"),
            (Address::from(pre::ARB_OWNER), "ArbOwner"),
            (Address::from(pre::NODE_INTERFACE), "NodeInterface"),
        ];

        for (addr, name) in predeploy_addresses {
            let precompile = Precompile::new(
                PrecompileId::custom(name),
                addr,
                move |input, gas_limit| {
                    // For now, create a minimal predeploy handler
                    // This is a simplified version that will be enhanced later
                    use crate::predeploys::PredeployRegistry;
                    use crate::predeploys::PredeployCallContext;
                    use crate::retryables::DefaultRetryables;

                    let mut registry = PredeployRegistry::with_default_addresses();
                    let mut retryables = DefaultRetryables::default();

                    // Create minimal context
                    // TODO: Get real context from EVM (block number, timestamp, etc.)
                    let ctx = PredeployCallContext {
                        block_number: 0,
                        block_hashes: alloc::vec::Vec::new(),
                        chain_id: alloy_primitives::U256::from(421614u64),
                        os_version: 0,
                        time: 0,
                        origin: Address::ZERO,
                        caller: Address::ZERO,
                        depth: 0,
                        basefee: alloy_primitives::U256::from(100_000_000u64), // 0.1 gwei
                    };

                    if let Some((output, gas_left, success)) = registry.dispatch(
                        &ctx,
                        addr,
                        &input,
                        gas_limit,
                        alloy_primitives::U256::ZERO,
                        &mut retryables,
                    ) {
                        if success {
                            let gas_used = gas_limit.saturating_sub(gas_left);
                            PrecompileResult::Ok(PrecompileOutput::new(gas_used, output))
                        } else {
                            PrecompileResult::Err(PrecompileErrors::Other(
                                "Predeploy execution failed".into()
                            ))
                        }
                    } else {
                        PrecompileResult::Err(PrecompileErrors::Other(
                            "Unknown predeploy address".into()
                        ))
                    }
                },
            );
            precompiles.extend([precompile]);
        }

        precompiles
    })
}
