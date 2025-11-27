//! X Layer chain initialization check utilities.

/// X Layer mainnet chain ID
const XLAYER_MAINNET_CHAIN_ID: u64 = 196;

/// X Layer testnet chain ID
const XLAYER_TESTNET_CHAIN_ID: u64 = 1952;

/// Checks if the given chain ID is for X Layer (mainnet or testnet) and was specified
/// via `--chain=xlayer-mainnet` or `--chain=xlayer-testnet` (built-in chain spec).
///
/// For X Layer chains, `init` command can only be used with a genesis file,
/// not with the built-in chain spec. This function returns an error only if:
/// 1. The chain ID matches X Layer mainnet or testnet
/// 2. The chain spec was specified via `--chain=xlayer-mainnet` or `--chain=xlayer-testnet`
///    (indicated by empty genesis alloc, which is the case for built-in chain specs)
///
/// # Arguments
///
/// * `chain_id` - The chain ID to check
/// * `has_empty_alloc` - Whether the genesis alloc is empty (true for built-in chain specs)
///
/// # Returns
///
/// Returns `Ok(())` if the chain ID is not X Layer, or if it's X Layer but specified via
/// genesis file. Returns an error if it's X Layer and specified via built-in chain name.
pub fn check_xlayer_init(chain_id: u64, has_empty_alloc: bool) -> eyre::Result<()> {
    if (chain_id == XLAYER_MAINNET_CHAIN_ID || chain_id == XLAYER_TESTNET_CHAIN_ID) &&
        has_empty_alloc
    {
        let chain_name =
            if chain_id == XLAYER_MAINNET_CHAIN_ID { "xlayer-mainnet" } else { "xlayer-testnet" };
        return Err(eyre::eyre!(format!(
            "Cannot use built-in chain spec for {chain_name}. Please use a genesis file instead.\n\
            Example: op-reth init --chain=/path/to/genesis.json"
        )));
    }
    Ok(())
}
