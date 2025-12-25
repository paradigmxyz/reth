//! ArbSys REVM precompile implementation.
//!
//! This module implements ArbSys as a proper REVM precompile that intercepts CALL operations
//! to address 0x64 and executes the ArbSys logic within the EVM's journal semantics.
//!
//! The precompile handles:
//! - SendTxToL1 (selector 0x928c169a): Send a transaction to L1
//! - WithdrawEth (selector 0x25e16063): Withdraw ETH to L1 (calls SendTxToL1 with empty data)
//!
//! Key operations:
//! 1. Compute send hash from transaction parameters
//! 2. Update Merkle accumulator state via sstore
//! 3. Emit SendMerkleUpdate events
//! 4. Burn call value from precompile's account
//! 5. Emit L2ToL1Tx event
//! 6. Return leaf number

use alloy_evm::precompiles::{DynPrecompile, PrecompileInput};
use alloy_primitives::{keccak256, Address, Bytes, Log, B256, U256};
use revm::precompile::{PrecompileId, PrecompileOutput, PrecompileResult, PrecompileError};

/// ArbSys precompile address (0x64 = 100)
pub const ARBSYS_ADDRESS: Address = Address::new([
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x64,
]);

/// ArbOS state address - the fictional account that stores ArbOS state
const ARBOS_STATE_ADDRESS: Address = Address::new([
    0xA4, 0xB0, 0x5F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF,
]);

/// SendTxToL1 function selector: keccak256("sendTxToL1(address,bytes)")[:4]
const SEND_TX_TO_L1_SELECTOR: [u8; 4] = [0x92, 0x8c, 0x16, 0x9a];

/// WithdrawEth function selector: keccak256("withdrawEth(address)")[:4]
const WITHDRAW_ETH_SELECTOR: [u8; 4] = [0x25, 0xe1, 0x60, 0x63];

/// L2ToL1Tx event topic: keccak256("L2ToL1Tx(address,address,uint256,uint256,uint256,uint256,uint256,uint256,bytes)")
fn l2_to_l1_tx_topic() -> B256 {
    keccak256(b"L2ToL1Tx(address,address,uint256,uint256,uint256,uint256,uint256,uint256,bytes)")
}

/// SendMerkleUpdate event topic: keccak256("SendMerkleUpdate(uint256,bytes32,uint256)")
fn send_merkle_update_topic() -> B256 {
    keccak256(b"SendMerkleUpdate(uint256,bytes32,uint256)")
}

/// Creates the ArbSys precompile as a DynPrecompile.
///
/// This precompile intercepts CALL operations to address 0x64 and executes
/// the ArbSys logic (SendTxToL1, WithdrawEth) within the EVM's journal semantics.
pub fn create_arbsys_precompile() -> DynPrecompile {
    (PrecompileId::custom("arbsys"), arbsys_precompile_handler).into()
}

/// Main precompile handler function.
fn arbsys_precompile_handler(mut input: PrecompileInput<'_>) -> PrecompileResult {
    let data = input.data;
    
    // Check minimum input length for selector
    if data.len() < 4 {
        return Err(PrecompileError::other("input too short"));
    }
    
    let selector: [u8; 4] = [data[0], data[1], data[2], data[3]];
    
    match selector {
        SEND_TX_TO_L1_SELECTOR => {
            handle_send_tx_to_l1(&mut input)
        }
        WITHDRAW_ETH_SELECTOR => {
            handle_withdraw_eth(&mut input)
        }
        _ => {
            // Unknown selector - revert
            Err(PrecompileError::other("unknown selector"))
        }
    }
}

/// Handle SendTxToL1 call.
/// Input format: selector (4 bytes) + destination (32 bytes) + offset (32 bytes) + length (32 bytes) + data
fn handle_send_tx_to_l1(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let data = input.data;
    
    // Parse destination address
    if data.len() < 4 + 32 {
        return Err(PrecompileError::other("input too short for destination"));
    }
    let mut dest_bytes = [0u8; 20];
    dest_bytes.copy_from_slice(&data[4 + 12..4 + 32]);
    let destination = Address::from(dest_bytes);
    
    // Parse calldata for L1
    let calldata_for_l1 = if data.len() >= 4 + 32 + 32 + 32 {
        // Read offset
        let mut offset_bytes = [0u8; 32];
        offset_bytes.copy_from_slice(&data[4 + 32..4 + 64]);
        let offset = U256::from_be_bytes(offset_bytes);
        let offset_usize: usize = offset.try_into().map_err(|_| PrecompileError::other("offset overflow"))?;
        
        // Read length
        if data.len() < 4 + offset_usize + 32 {
            return Err(PrecompileError::other("input too short for length"));
        }
        let mut len_bytes = [0u8; 32];
        len_bytes.copy_from_slice(&data[4 + offset_usize..4 + offset_usize + 32]);
        let len = U256::from_be_bytes(len_bytes);
        let len_usize: usize = len.try_into().map_err(|_| PrecompileError::other("length overflow"))?;
        
        // Read data
        if data.len() < 4 + offset_usize + 32 + len_usize {
            return Err(PrecompileError::other("input too short for data"));
        }
        Bytes::copy_from_slice(&data[4 + offset_usize + 32..4 + offset_usize + 32 + len_usize])
    } else {
        Bytes::new()
    };
    
    execute_send_tx_to_l1(input, destination, calldata_for_l1)
}

/// Handle WithdrawEth call.
/// Input format: selector (4 bytes) + destination (32 bytes)
fn handle_withdraw_eth(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let data = input.data;
    
    // Parse destination address
    if data.len() < 4 + 32 {
        return Err(PrecompileError::other("input too short for destination"));
    }
    let mut dest_bytes = [0u8; 20];
    dest_bytes.copy_from_slice(&data[4 + 12..4 + 32]);
    let destination = Address::from(dest_bytes);
    
    // WithdrawEth is just SendTxToL1 with empty calldata
    execute_send_tx_to_l1(input, destination, Bytes::new())
}

/// Execute the SendTxToL1 logic.
fn execute_send_tx_to_l1(
    input: &mut PrecompileInput<'_>,
    destination: Address,
    calldata_for_l1: Bytes,
) -> PrecompileResult {
    let caller = input.caller;
    let value = input.value;
    let internals = input.internals_mut();
    
    // Get block info
    let block_number = internals.block_number();
    let block_timestamp = internals.block_timestamp();
    
    // Get L1 block number from ArbOS state
    // Storage layout: blockhashes substorage at offset 5, l1BlockNumber at offset 0
    let l1_block_number = get_l1_block_number(internals)?;
    
    tracing::info!(
        target: "arb::arbsys_precompile",
        caller = ?caller,
        destination = ?destination,
        value = %value,
        block_number = %block_number,
        l1_block_number = l1_block_number,
        timestamp = %block_timestamp,
        calldata_len = calldata_for_l1.len(),
        "SendTxToL1 called"
    );
    
    // Compute send hash (matches Go nitro's arbosState.KeccakHash)
    let send_hash = compute_send_hash(
        &caller,
        &destination,
        block_number.try_into().unwrap_or(0),
        l1_block_number,
        block_timestamp.try_into().unwrap_or(0),
        &value,
        &calldata_for_l1,
    );
    
    // Append to Merkle accumulator and get events
    let (leaf_num, merkle_events) = append_to_merkle_accumulator(internals, send_hash)?;
    
    tracing::info!(
        target: "arb::arbsys_precompile",
        leaf_num = leaf_num,
        send_hash = ?send_hash,
        merkle_events_count = merkle_events.len(),
        "Merkle accumulator updated"
    );
    
    // Burn the call value from the precompile's account
    // The EVM has already transferred the value from caller to ArbSys (0x64)
    // We need to burn it (subtract from ArbSys balance)
    if value > U256::ZERO {
        burn_value_from_precompile(internals, value)?;
    }
    
    // Emit SendMerkleUpdate events
    let send_merkle_update_topic = send_merkle_update_topic();
    for event in &merkle_events {
        let position = level_and_leaf_to_u256(event.level, event.num_leaves);
        
        // ABI encode: (reserved=0, hash, position)
        let mut event_data = Vec::with_capacity(96);
        event_data.extend_from_slice(&U256::ZERO.to_be_bytes::<32>());
        event_data.extend_from_slice(event.hash.as_slice());
        event_data.extend_from_slice(&position.to_be_bytes::<32>());
        
        let log = Log::new(
            ARBSYS_ADDRESS,
            vec![send_merkle_update_topic],
            event_data.into(),
        ).expect("valid log");
        
        internals.log(log);
    }
    
    // Emit L2ToL1Tx event
    emit_l2_to_l1_tx_event(
        internals,
        &caller,
        &destination,
        &send_hash,
        leaf_num,
        block_number.try_into().unwrap_or(0),
        l1_block_number,
        block_timestamp.try_into().unwrap_or(0),
        &value,
        &calldata_for_l1,
    );
    
    // Return leaf_num as the result (ABI encoded as uint256)
    let return_value = U256::from(leaf_num);
    let output = return_value.to_be_bytes_vec();
    
    // Gas cost - use a reasonable estimate based on operations performed
    // Go nitro charges based on the precompile's gas model
    let gas_used = calculate_gas_cost(calldata_for_l1.len(), merkle_events.len());
    
    Ok(PrecompileOutput::new(gas_used, output.into()))
}

/// Get L1 block number from ArbOS state.
fn get_l1_block_number(internals: &mut alloy_evm::EvmInternals<'_>) -> Result<u64, PrecompileError> {
    // Storage layout in Go nitro:
    // - ArbOS state is at ARBOS_STATE_ADDRESS
    // - blockhashes substorage is at subspace ID 6 (blockhashesSubspace = []byte{6})
    // - l1BlockNumber is at offset 0 within blockhashes substorage
    
    // IMPORTANT: Must load the account into the journal before accessing storage
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // Compute the storage slot for blockhashes.l1BlockNumber
    // This matches Go nitro's storage.OpenSubStorage and storage.NewSlot
    // blockhashesSubspace = []byte{6} in Go nitro
    let blockhashes_key = compute_substorage_key(&[], 6);
    let l1_block_slot = compute_storage_slot(&blockhashes_key, 0);
    
    let result = internals.sload(ARBOS_STATE_ADDRESS, l1_block_slot)
        .map_err(|e| PrecompileError::other(format!("sload failed: {:?}", e)))?;
    
    let value: u64 = result.data.try_into().unwrap_or(0);
    Ok(value)
}

/// Compute send hash (matches Go nitro's arbosState.KeccakHash).
fn compute_send_hash(
    caller: &Address,
    destination: &Address,
    block_number: u64,
    l1_block_number: u64,
    timestamp: u64,
    value: &U256,
    calldata_for_l1: &Bytes,
) -> B256 {
    // Go nitro computes: keccak256(caller, destination, blockNumber, l1BlockNum, timestamp, value, calldataForL1)
    // Addresses are 20 bytes (NOT padded), numbers are 32 bytes (U256 format)
    let mut data = Vec::with_capacity(20 + 20 + 32 * 4 + calldata_for_l1.len());
    
    // caller (20 bytes - NOT padded!)
    data.extend_from_slice(caller.as_slice());
    
    // destination (20 bytes - NOT padded!)
    data.extend_from_slice(destination.as_slice());
    
    // blockNumber (U256 - 32 bytes)
    data.extend_from_slice(&U256::from(block_number).to_be_bytes::<32>());
    
    // l1BlockNum (U256 - 32 bytes)
    data.extend_from_slice(&U256::from(l1_block_number).to_be_bytes::<32>());
    
    // timestamp (U256 - 32 bytes)
    data.extend_from_slice(&U256::from(timestamp).to_be_bytes::<32>());
    
    // value (U256 - 32 bytes)
    data.extend_from_slice(&value.to_be_bytes::<32>());
    
    // calldataForL1 (raw bytes)
    data.extend_from_slice(calldata_for_l1);
    
    keccak256(&data)
}

/// Merkle tree node event (matches Go nitro's MerkleTreeNodeEvent)
struct MerkleTreeNodeEvent {
    level: u64,
    num_leaves: u64,
    hash: B256,
}

/// Append to Merkle accumulator and return (leaf_num, events).
fn append_to_merkle_accumulator(
    internals: &mut alloy_evm::EvmInternals<'_>,
    item_hash: B256,
) -> Result<(u64, Vec<MerkleTreeNodeEvent>), PrecompileError> {
    // Storage layout for SendMerkleAccumulator:
    // - Substorage at subspace ID 5 (sendMerkleSubspace = []byte{5})
    // - Size at offset 0 within substorage
    // - Partials at offset 2+ within substorage
    
    // IMPORTANT: Ensure account is loaded (should already be from get_l1_block_number, but be safe)
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // sendMerkleSubspace = []byte{5} in Go nitro
    let merkle_key = compute_substorage_key(&[], 5);
    let size_slot = compute_storage_slot(&merkle_key, 0);
    
    // Get current size
    let current_size_result = internals.sload(ARBOS_STATE_ADDRESS, size_slot)
        .map_err(|e| PrecompileError::other(format!("sload size failed: {:?}", e)))?;
    let current_size: u64 = current_size_result.data.try_into().unwrap_or(0);
    let new_size = current_size + 1;
    
    // Set new size
    internals.sstore(ARBOS_STATE_ADDRESS, size_slot, U256::from(new_size))
        .map_err(|e| PrecompileError::other(format!("sstore size failed: {:?}", e)))?;
    
    let mut events = Vec::new();
    let mut level = 0u64;
    let mut so_far = keccak256(item_hash.as_slice());
    
    loop {
        if level == calc_num_partials(current_size) {
            set_partial(internals, &merkle_key, level, so_far)?;
            break;
        }
        
        let this_level = get_partial(internals, &merkle_key, level)?;
        if this_level == B256::ZERO {
            set_partial(internals, &merkle_key, level, so_far)?;
            break;
        }
        
        // Combine hashes
        let mut combined = Vec::with_capacity(64);
        combined.extend_from_slice(this_level.as_slice());
        combined.extend_from_slice(so_far.as_slice());
        so_far = keccak256(&combined);
        
        // Clear this level
        set_partial(internals, &merkle_key, level, B256::ZERO)?;
        
        level += 1;
        
        // Event is emitted AFTER incrementing level
        events.push(MerkleTreeNodeEvent {
            level,
            num_leaves: new_size - 1,
            hash: so_far,
        });
    }
    
    let leaf_num = new_size - 1;
    Ok((leaf_num, events))
}

/// Calculate number of partials for a given size.
fn calc_num_partials(size: u64) -> u64 {
    if size == 0 {
        return 0;
    }
    64 - (size - 1).leading_zeros() as u64
}

/// Get partial at level.
fn get_partial(
    internals: &mut alloy_evm::EvmInternals<'_>,
    merkle_key: &[u8],
    level: u64,
) -> Result<B256, PrecompileError> {
    let slot = compute_storage_slot(merkle_key, 2 + level);
    let result = internals.sload(ARBOS_STATE_ADDRESS, slot)
        .map_err(|e| PrecompileError::other(format!("sload partial failed: {:?}", e)))?;
    Ok(B256::from(result.data))
}

/// Set partial at level.
fn set_partial(
    internals: &mut alloy_evm::EvmInternals<'_>,
    merkle_key: &[u8],
    level: u64,
    value: B256,
) -> Result<(), PrecompileError> {
    let slot = compute_storage_slot(merkle_key, 2 + level);
    let value_u256 = U256::from_be_bytes(value.0);
    internals.sstore(ARBOS_STATE_ADDRESS, slot, value_u256)
        .map_err(|e| PrecompileError::other(format!("sstore partial failed: {:?}", e)))?;
    Ok(())
}

/// Burn value from precompile's account.
fn burn_value_from_precompile(
    internals: &mut alloy_evm::EvmInternals<'_>,
    value: U256,
) -> Result<(), PrecompileError> {
    // Load the ArbSys account
    let account_result = internals.load_account(ARBSYS_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    let account = account_result.data;
    
    // Check balance
    if account.info.balance < value {
        return Err(PrecompileError::other("insufficient balance to burn"));
    }
    
    // Subtract value (burn)
    account.info.balance = account.info.balance.saturating_sub(value);
    
    tracing::info!(
        target: "arb::arbsys_precompile",
        value = %value,
        new_balance = %account.info.balance,
        "Burned value from ArbSys account"
    );
    
    Ok(())
}

/// Emit L2ToL1Tx event.
fn emit_l2_to_l1_tx_event(
    internals: &mut alloy_evm::EvmInternals<'_>,
    caller: &Address,
    destination: &Address,
    send_hash: &B256,
    leaf_num: u64,
    block_number: u64,
    l1_block_number: u64,
    timestamp: u64,
    value: &U256,
    calldata_for_l1: &Bytes,
) {
    let l2_to_l1_tx_topic = l2_to_l1_tx_topic();
    
    // Indexed topics: destination, hash, position
    let mut dest_topic = [0u8; 32];
    dest_topic[12..].copy_from_slice(destination.as_slice());
    
    let hash_topic = send_hash.0;
    let position_topic = U256::from(leaf_num).to_be_bytes::<32>();
    
    // Non-indexed data: caller, arbBlockNum, ethBlockNum, timestamp, callvalue, data
    let mut event_data = Vec::new();
    
    // caller (address padded to 32 bytes)
    let mut caller_bytes = [0u8; 32];
    caller_bytes[12..].copy_from_slice(caller.as_slice());
    event_data.extend_from_slice(&caller_bytes);
    
    // arbBlockNum (uint256)
    event_data.extend_from_slice(&U256::from(block_number).to_be_bytes::<32>());
    
    // ethBlockNum (uint256)
    event_data.extend_from_slice(&U256::from(l1_block_number).to_be_bytes::<32>());
    
    // timestamp (uint256)
    event_data.extend_from_slice(&U256::from(timestamp).to_be_bytes::<32>());
    
    // callvalue (uint256)
    event_data.extend_from_slice(&value.to_be_bytes::<32>());
    
    // data (bytes) - ABI encoded with offset, length, and padded data
    // Offset to data: 6 * 32 = 192 = 0xc0
    event_data.extend_from_slice(&U256::from(192u64).to_be_bytes::<32>());
    
    // Length of data
    event_data.extend_from_slice(&U256::from(calldata_for_l1.len()).to_be_bytes::<32>());
    
    // Data bytes (padded to 32-byte boundary)
    event_data.extend_from_slice(calldata_for_l1);
    let padding_len = (32 - (calldata_for_l1.len() % 32)) % 32;
    event_data.extend(core::iter::repeat(0u8).take(padding_len));
    
    let log = Log::new(
        ARBSYS_ADDRESS,
        vec![l2_to_l1_tx_topic, B256::from(dest_topic), B256::from(hash_topic), B256::from(position_topic)],
        event_data.into(),
    ).expect("valid log");
    
    internals.log(log);
}

/// Compute substorage key (matches Go nitro's storage.OpenSubStorage).
/// In Go nitro, subspace IDs are single bytes like []byte{5} for sendMerkle, []byte{6} for blockhashes.
fn compute_substorage_key(base_key: &[u8], subspace_id: u8) -> Vec<u8> {
    // Go nitro: keccak256(baseKey || subspaceId)
    // subspaceId is a single byte, not a u64
    let mut data = Vec::with_capacity(base_key.len() + 1);
    data.extend_from_slice(base_key);
    data.push(subspace_id);
    keccak256(&data).to_vec()
}

/// Compute storage slot (matches Go nitro's storage.NewSlot / storage_key_map).
fn compute_storage_slot(storage_key: &[u8], offset: u64) -> U256 {
    let boundary = 31usize;
    
    // Convert offset to a 32-byte key (BE format with the offset in the last 8 bytes)
    let mut key_bytes = [0u8; 32];
    key_bytes[24..32].copy_from_slice(&offset.to_be_bytes());
    
    let mut data = Vec::with_capacity(storage_key.len() + boundary);
    data.extend_from_slice(storage_key);
    data.extend_from_slice(&key_bytes[..boundary]);
    let h = keccak256(&data);
    let mut mapped = [0u8; 32];
    mapped[..boundary].copy_from_slice(&h.0[..boundary]);
    mapped[boundary] = key_bytes[boundary];
    U256::from_be_bytes(mapped)
}

/// Compute LevelAndLeaf position as U256.
fn level_and_leaf_to_u256(level: u64, leaf: u64) -> U256 {
    let level_shifted = U256::from(level) << 192;
    level_shifted + U256::from(leaf)
}

/// Gas constants matching Go nitro's storage/storage.go and Ethereum params
const STORAGE_READ_COST: u64 = 800;  // params.SloadGasEIP2200
const STORAGE_WRITE_COST: u64 = 20000;  // params.SstoreSetGasEIP2200 (0 -> non-zero)
const STORAGE_WRITE_ZERO_COST: u64 = 5000;  // params.SstoreResetGasEIP2200 (non-zero -> zero)
const LOG_GAS: u64 = 375;  // params.LogGas
const LOG_TOPIC_GAS: u64 = 375;  // params.LogTopicGas
const LOG_DATA_GAS: u64 = 8;  // params.LogDataGas
const COPY_GAS: u64 = 3;  // params.CopyGas

/// Calculate gas cost for the precompile matching Go nitro's approach.
/// 
/// Go nitro charges gas via the burner for:
/// 1. Input args copying: CopyGas * WordsForBytes(len(input)-4)
/// 2. Storage reads: SloadGasEIP2200 per sload
/// 3. Storage writes: SstoreSetGasEIP2200 or SstoreResetGasEIP2200 per sstore
/// 4. Keccak operations: 30 + 6 * words
/// 5. Event emissions: LogGas + LogTopicGas * (1 + indexed_count) + LogDataGas * data_len
/// 6. Output result copying: CopyGas * WordsForBytes(len(output))
fn calculate_gas_cost(calldata_for_l1_len: usize, merkle_events_count: usize) -> u64 {
    // Input args copying cost (for withdrawEth: 32 bytes = 1 word)
    // Go nitro: argsCost := params.CopyGas * arbmath.WordsForBytes(uint64(len(input)-4))
    let args_words = (32 + 31) / 32;  // 1 word for the address argument
    let args_cost = COPY_GAS * args_words;
    
    // Storage operations for get_l1_block_number:
    // - 1 sload for L1 block number
    let l1_block_read_cost = STORAGE_READ_COST;
    
    // Keccak for send hash computation
    // Go nitro: cost := 30 + 6*arbmath.WordsForBytes(byteCount)
    // sendHash uses: caller(20) + dest(20) + blockNum(32) + l1BlockNum(32) + timestamp(32) + value(32) + calldata
    let send_hash_bytes = 20 + 20 + 32 + 32 + 32 + 32 + calldata_for_l1_len;
    let send_hash_words = (send_hash_bytes as u64 + 31) / 32;
    let keccak_cost = 30 + 6 * send_hash_words;
    
    // Storage operations for Merkle accumulator:
    // - 1 sload for current size
    // - 1 sstore for new size
    // - For each level in the tree: 1 sload for partial, potentially 1 sstore
    // In the worst case (first append), we have 1 sload + 1 sstore for size, 1 sload + 1 sstore for partial[0]
    // For subsequent appends, we may have more levels
    let merkle_size_read_cost = STORAGE_READ_COST;
    let merkle_size_write_cost = STORAGE_WRITE_COST;
    
    // For each merkle event, there's a level that got updated
    // Each level involves: 1 sload for partial, 1 sstore to clear it, then 1 sstore at next level
    // Plus keccak for combining hashes
    let merkle_level_cost = if merkle_events_count > 0 {
        // Each event means we combined two hashes and wrote to a higher level
        // sload for partial at each level, sstore to clear, keccak to combine
        let per_level_cost = STORAGE_READ_COST + STORAGE_WRITE_ZERO_COST + (30 + 6 * 2);  // 2 words for 64 bytes
        (merkle_events_count as u64) * per_level_cost
    } else {
        0
    };
    
    // Final partial write (always happens)
    let final_partial_read_cost = STORAGE_READ_COST;
    let final_partial_write_cost = STORAGE_WRITE_COST;
    
    // Initial hash of item (keccak of sendHash)
    let initial_hash_cost = 30 + 6 * 1;  // 1 word for 32 bytes
    
    // L2ToL1Tx event gas cost
    // Event signature from ArbSys.sol:
    //   event L2ToL1Tx(
    //     address caller,              // non-indexed
    //     address indexed destination, // indexed
    //     uint256 indexed hash,        // indexed
    //     uint256 indexed position,    // indexed
    //     uint256 arbBlockNum,         // non-indexed
    //     uint256 ethBlockNum,         // non-indexed
    //     uint256 timestamp,           // non-indexed
    //     uint256 callvalue,           // non-indexed
    //     bytes data                   // non-indexed
    //   );
    // Go nitro: cost := params.LogGas + params.LogTopicGas * uint64(1+len(topicInputs)) + params.LogDataGas * uint64(len(data))
    let l2_to_l1_tx_indexed_count = 3u64;  // destination, hash, position
    // Non-indexed data is ABI-encoded: caller(32) + arbBlockNum(32) + ethBlockNum(32) + timestamp(32) + callvalue(32) + offset(32) + length(32) + data(padded)
    // For empty calldata: 7 * 32 = 224 bytes
    let l2_to_l1_tx_data_len = 32 * 6 + 32 + ((calldata_for_l1_len + 31) / 32) * 32;  // 6 fixed fields + offset + data
    let l2_to_l1_tx_log_cost = LOG_GAS + LOG_TOPIC_GAS * (1 + l2_to_l1_tx_indexed_count) + LOG_DATA_GAS * (l2_to_l1_tx_data_len as u64);
    
    // SendMerkleUpdate events (one per merkle_events_count)
    // Event has: reserved(indexed), hash(indexed), position(indexed) - 3 indexed, 0 non-indexed
    let send_merkle_update_indexed_count = 3;
    let send_merkle_update_log_cost = LOG_GAS + LOG_TOPIC_GAS * (1 + send_merkle_update_indexed_count);
    let total_merkle_update_log_cost = (merkle_events_count as u64) * send_merkle_update_log_cost;
    
    // Output result copying cost (32 bytes = 1 word for leafNum)
    let result_words = 1u64;
    let result_cost = COPY_GAS * result_words;
    
    // Total gas
    args_cost
        + l1_block_read_cost
        + keccak_cost
        + merkle_size_read_cost
        + merkle_size_write_cost
        + merkle_level_cost
        + final_partial_read_cost
        + final_partial_write_cost
        + initial_hash_cost
        + l2_to_l1_tx_log_cost
        + total_merkle_update_log_cost
        + result_cost
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_selectors() {
        // Verify selectors match expected values
        let send_tx_selector = keccak256(b"sendTxToL1(address,bytes)");
        assert_eq!(&send_tx_selector[0..4], &SEND_TX_TO_L1_SELECTOR);
        
        let withdraw_eth_selector = keccak256(b"withdrawEth(address)");
        assert_eq!(&withdraw_eth_selector[0..4], &WITHDRAW_ETH_SELECTOR);
    }
    
    #[test]
    fn test_compute_send_hash() {
        let caller = Address::ZERO;
        let destination = Address::ZERO;
        let hash = compute_send_hash(
            &caller,
            &destination,
            100,
            50,
            1234567890,
            &U256::from(1000u64),
            &Bytes::from(vec![0x01, 0x02, 0x03]),
        );
        assert_ne!(hash, B256::ZERO);
    }
    
    #[test]
    fn test_level_and_leaf_to_u256() {
        // level=1, leaf=0 should give (1 << 192) + 0
        let result = level_and_leaf_to_u256(1, 0);
        let expected = U256::from(1u64) << 192;
        assert_eq!(result, expected);
        
        // level=0, leaf=5 should give 5
        let result = level_and_leaf_to_u256(0, 5);
        assert_eq!(result, U256::from(5u64));
    }
    
    #[test]
    fn test_calc_num_partials() {
        assert_eq!(calc_num_partials(0), 0);
        assert_eq!(calc_num_partials(1), 1);
        assert_eq!(calc_num_partials(2), 1);
        assert_eq!(calc_num_partials(3), 2);
        assert_eq!(calc_num_partials(4), 2);
        assert_eq!(calc_num_partials(5), 3);
    }
}
