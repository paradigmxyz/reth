use alloy_primitives::{Address, Bytes, ChainId, TxHash, U256};
use reth_codecs::{Compact, Decode, Encode};
use serde::{Deserialize, Serialize};

/// Hyperliquid deposit transaction from 0x222...22
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Compact)]
pub struct DepositTransaction {
    /// Chain ID
    pub chain_id: ChainId,
    /// Recipient address
    pub to: Address,
    /// Deposit amount
    pub value: U256,
    /// Deposit data/memo
    pub data: Bytes,
    /// Block number where deposit occurred
    pub block_number: u64,
    /// Transaction index in block
    pub transaction_index: u64,
}

impl DepositTransaction {
    /// The deposit contract address (0x222...22)
    pub const DEPOSIT_CONTRACT: Address = Address::repeat_byte(0x22);
    
    /// Convert to pseudo transaction hash
    pub fn pseudo_hash(&self) -> TxHash {
        use alloy_primitives::keccak256;
        let encoded = alloy_rlp::encode(self);
        keccak256(encoded).into()
    }
    
    /// Check if this is a deposit transaction
    pub fn is_deposit_transaction(from: Address) -> bool {
        from == Self::DEPOSIT_CONTRACT
    }
}

/// Extended transaction type that includes pseudo deposits
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyperliquidTransaction {
    /// Standard Ethereum transaction
    Standard(Transaction),
    /// Hyperliquid deposit pseudo-transaction
    Deposit(DepositTransaction),
}