//! OP stack variation of chain spec constants.

use alloy_primitives::{b256, hex, B256};

//-------------------------------- OP MAINNET --------------------------------//

/// Optimism Mainnet genesish hash at bedrock block.
///
/// NOTE: For OP mainnet genesis header can't be properly computed from the genesis json file due
/// to OVM history.
pub const OP_MAINNET_GENESIS_HASH: B256 =
    b256!("0x7ca38a1916c42007829c55e69d3e9a73265554b586a499015373241b8a3fa48b");

//-------------------------------- OP SEPOLIA --------------------------------//

/// Optimism Sepolia genesis hash.
pub const OP_SEPOLIA_GENESIS_HASH: B256 =
    b256!("0x102de6ffb001480cc9b8b548fd05c34cd4f46ae4aa91759393db90ea0409887d");

//------------------------------- BASE MAINNET -------------------------------//

/// Base Mainnet genesis hash.
pub const BASE_MAINNET_GENESIS_HASH: B256 =
    b256!("0xf712aa9241cc24369b143cf6dce85f0902a9731e70d66818a3a5845b296c73dd");

/// Max gas limit on Base: <https://basescan.org/block/17208876>
pub const BASE_MAINNET_MAX_GAS_LIMIT: u64 = 105_000_000;

//------------------------------- BASE SEPOLIA -------------------------------//

/// Base Sepolia genesis hash.
pub const BASE_SEPOLIA_GENESIS_HASH: B256 =
    b256!("0x0dcc9e089e30b90ddfc55be9a37dd15bc551aeee999d2e2b51414c54eaf934e4");

/// Max gas limit on Base Sepolia: <https://sepolia.basescan.org/block/12506483>
pub const BASE_SEPOLIA_MAX_GAS_LIMIT: u64 = 45_000_000;

//----------------------------------- DEV ------------------------------------//

/// Dummy system transaction for dev mode
/// OP Mainnet transaction at index 0 in block 124665056.
///
/// <https://optimistic.etherscan.io/tx/0x312e290cf36df704a2217b015d6455396830b0ce678b860ebfcc30f41403d7b1>
pub const TX_SET_L1_BLOCK_OP_MAINNET_BLOCK_124665056: [u8; 251] = hex!("7ef8f8a0683079df94aa5b9cf86687d739a60a9b4f0835e520ec4d664e2e415dca17a6df94deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b8a4440a5e200000146b000f79c500000000000000040000000066d052e700000000013ad8a3000000000000000000000000000000000000000000000000000000003ef1278700000000000000000000000000000000000000000000000000000000000000012fdf87b89884a61e74b322bbcf60386f543bfae7827725efaaf0ab1de2294a590000000000000000000000006887246668a3b87f54deb3b94ba47a6f63f32985");
