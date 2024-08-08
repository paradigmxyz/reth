//! Taiko related functionality for the block executor.

use anyhow::{anyhow, bail, ensure, Context, Result};
use lazy_static::lazy_static;
use reth_primitives::{Block, Header, TransactionSigned, TxKind};
use revm_primitives::{alloy_primitives::uint, Address, U256};
use std::str::FromStr;

/// Data required to validate a Taiko Block
#[derive(Clone, Debug, Default)]
pub struct TaikoData {
    /// header
    pub l1_header: Header,
    /// parent L1 header
    pub parent_header: Header,
    /// L2 contract
    pub l2_contract: Address,
    /// base fee sharing ratio
    pub basefee_ratio: u8,
    /// base fee adjustment quotient
    pub basefee_adj_quotient: u8,
    /// gas issuance per second
    pub gas_issue_per_sec: u32,
}

/// Anchor tx gas limit
pub const ANCHOR_GAS_LIMIT: u64 = 250_000;

lazy_static! {
    /// The address calling the anchor transaction
    pub static ref GOLDEN_TOUCH_ACCOUNT: Address = {
        Address::from_str("0x0000777735367b36bC9B61C50022d9D0700dB4Ec")
            .expect("invalid golden touch account")
    };
    static ref GX1: U256 =
        uint!(0x79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798_U256);
    static ref N: U256 =
        uint!(0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141_U256);
    static ref GX1_MUL_PRIVATEKEY: U256 =
        uint!(0x4341adf5a780b4a87939938fd7a032f6e6664c7da553c121d3b4947429639122_U256);
    static ref GX2: U256 =
        uint!(0xc6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5_U256);
}

/// check the anchor signature with fixed K value
fn check_anchor_signature(anchor: &TransactionSigned) -> Result<()> {
    let sign = anchor.signature();
    if sign.r == *GX1 {
        return Ok(());
    }
    let msg_hash = anchor.signature_hash();
    let msg_hash: U256 = msg_hash.into();
    if sign.r == *GX2 {
        // when r == GX2 require s == 0 if k == 1
        // alias: when r == GX2 require N == msg_hash + *GX1_MUL_PRIVATEKEY
        if *N != msg_hash + *GX1_MUL_PRIVATEKEY {
            bail!(
                "r == GX2, but N != msg_hash + *GX1_MUL_PRIVATEKEY, N: {}, msg_hash: {msg_hash}, *GX1_MUL_PRIVATEKEY: {}",
                *N, *GX1_MUL_PRIVATEKEY
            );
        }
        return Ok(());
    }
    Err(anyhow!("r != *GX1 && r != GX2, r: {}, *GX1: {}, GX2: {}", sign.r, *GX1, *GX2))
}

use alloy_sol_types::{sol, SolCall};

sol! {
    /// Anchor call
    function anchor(
        /// The L1 hash
        bytes32 l1Hash,
        /// The L1 state root
        bytes32 l1StateRoot,
        /// The L1 block number
        uint64 l1BlockId,
        /// The gas used in the parent block
        uint32 parentGasUsed
    )
        external
    {}

    function anchorV2(
        uint64 _anchorBlockId,
        bytes32 _anchorStateRoot,
        uint32 _parentGasUsed,
        uint32 _gasIssuancePerSecond,
        uint8 _basefeeAdjustmentQuotient
    )
        external
        nonReentrant
    {}
}

// todo, use compiled abi once test passes
// sol!(TaikoL2, "./res/TaikoL2.json");
// use TaikoL2::{anchor, anchorV2};

/// Decode anchor tx data
pub fn decode_anchor(bytes: &[u8]) -> Result<anchorCall> {
    anchorCall::abi_decode(bytes, true).map_err(|e| anyhow!(e))
}

/// Verifies the anchor tx correctness
pub fn check_anchor_tx(
    tx: &TransactionSigned,
    from: &Address,
    block: &Block,
    taiko_data: TaikoData,
) -> Result<()> {
    let anchor: &reth_primitives::TxEip1559 =
        tx.as_eip1559().context(anyhow!("anchor tx is not an EIP1559 tx"))?;

    // Check the signature
    check_anchor_signature(tx).context(anyhow!("failed to check anchor signature"))?;

    // Extract the `to` address
    let TxKind::Call(to) = anchor.to else { panic!("anchor tx not a smart contract call") };
    // Check that it's from the golden touch address
    ensure!(*from == *GOLDEN_TOUCH_ACCOUNT, "anchor transaction from mismatch");
    // Check that the L2 contract is being called
    ensure!(to == taiko_data.l2_contract, "anchor transaction to mismatch");
    // Tx can't have any ETH attached
    ensure!(anchor.value == U256::from(0), "anchor transaction value mismatch");
    // Tx needs to have the expected gas limit
    ensure!(anchor.gas_limit == ANCHOR_GAS_LIMIT, "anchor transaction gas price mismatch");
    // Check needs to have the base fee set to the block base fee
    ensure!(
        anchor.max_fee_per_gas == block.header.base_fee_per_gas.unwrap().into(),
        "anchor transaction gas mismatch"
    );

    // Okay now let's decode the anchor tx to verify the inputs
    let anchor_call = decode_anchor(&anchor.input)?;
    // The L1 blockhash needs to match the expected value
    ensure!(anchor_call.l1Hash == taiko_data.l1_header.hash_slow(), "L1 hash mismatch");
    ensure!(anchor_call.l1StateRoot == taiko_data.l1_header.state_root, "L1 state root mismatch");
    ensure!(anchor_call.l1BlockId == taiko_data.l1_header.number, "L1 block number mismatch");
    // The parent gas used input needs to match the gas used value of the parent block
    ensure!(
        anchor_call.parentGasUsed == taiko_data.parent_header.gas_used as u32,
        "parentGasUsed mismatch"
    );

    Ok(())
}

pub fn decode_anchor_ontake(bytes: &[u8]) -> Result<anchorV2Call> {
    anchorV2Call::abi_decode(bytes, true).map_err(|e| anyhow!(e))
}

/// Verifies the anchor tx correctness in ontake fork
pub fn check_anchor_tx_ontake(
    tx: &TransactionSigned,
    from: &Address,
    block: &Block,
    taiko_data: TaikoData,
) -> Result<()> {
    let anchor: &reth_primitives::TxEip1559 =
        tx.as_eip1559().context(anyhow!("anchor tx is not an EIP1559 tx"))?;

    // Check the signature
    check_anchor_signature(tx).context(anyhow!("failed to check anchor signature"))?;

    // Extract the `to` address
    let TxKind::Call(to) = anchor.to else { panic!("anchor tx not a smart contract call") };
    // Check that it's from the golden touch address
    ensure!(*from == *GOLDEN_TOUCH_ACCOUNT, "anchor transaction from mismatch");
    // Check that the L2 contract is being called
    ensure!(to == taiko_data.l2_contract, "anchor transaction to mismatch");
    // Tx can't have any ETH attached
    ensure!(anchor.value == U256::from(0), "anchor transaction value mismatch");
    // Tx needs to have the expected gas limit
    ensure!(anchor.gas_limit == ANCHOR_GAS_LIMIT, "anchor transaction gas price mismatch");
    // Check needs to have the base fee set to the block base fee
    ensure!(
        anchor.max_fee_per_gas == block.header.base_fee_per_gas.unwrap().into(),
        "anchor transaction gas mismatch"
    );

    /*
        uint64 _anchorBlockId,
        bytes32 _anchorStateRoot,
        uint32 _parentGasUsed,
        uint32 _gasIssuancePerSecond,
        uint8 _basefeeAdjustmentQuotient
    */

    // Okay now let's decode the anchor tx to verify the inputs
    let anchor_call = decode_anchor_ontake(&anchor.input)?;
    ensure!(
        anchor_call._anchorStateRoot == taiko_data.l1_header.state_root,
        "L1 state root mismatch"
    );
    ensure!(anchor_call._anchorBlockId == taiko_data.l1_header.number, "L1 block number mismatch");
    ensure!(
        anchor_call._anchorStateRoot == taiko_data.l1_header.state_root,
        "L1 state root mismatch"
    );
    // The parent gas used input needs to match the gas used value of the parent block
    ensure!(
        anchor_call._parentGasUsed == taiko_data.parent_header.gas_used as u32,
        "parentGasUsed mismatch"
    );
    ensure!(
        anchor_call._gasIssuancePerSecond == taiko_data.gas_issue_per_sec,
        "gas issuance per second mismatch"
    );
    ensure!(
        anchor_call._basefeeAdjustmentQuotient == taiko_data.basefee_adj_quotient,
        "basefee adjustment quotient mismatch"
    );

    Ok(())
}
