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
use std::sync::atomic::{AtomicU64, Ordering};
use std::cell::RefCell;

/// Global execution counter to track which execution pass the precompile is called in.
static EXEC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Represents the state changes from an ArbSys precompile call that need to be applied
/// after EVM execution using the proper transition mechanism.
#[derive(Debug, Clone, Default)]
pub struct ArbSysMerkleState {
    /// The new size of the Merkle accumulator
    pub new_size: u64,
    /// Partial hashes to set: (level, hash)
    pub partials: Vec<(u64, B256)>,
    /// The send hash for this transaction
    pub send_hash: B256,
    /// The leaf number for this transaction
    pub leaf_num: u64,
    /// The value to burn from ArbSys account
    pub value_to_burn: U256,
    /// Block number this state change is for (for shadow accumulator tracking)
    pub block_number: u64,
}

/// Block-local shadow accumulator state.
/// This tracks the cumulative Merkle state across transactions within the same block.
/// The precompile reads from this shadow first, so tx2 can see tx1's updated state.
#[derive(Debug, Clone, Default)]
pub struct ShadowAccumulatorState {
    /// Current size of the Merkle accumulator (cumulative across txs in block)
    pub size: u64,
    /// Partial hashes at each level
    pub partials: Vec<B256>,
    /// Block number this shadow is for (to detect block boundaries)
    pub block_number: u64,
}

thread_local! {
    /// Thread-local storage for ArbSys state changes.
    /// This is set by the precompile and consumed by build.rs after EVM execution.
    static ARBSYS_STATE: RefCell<Option<ArbSysMerkleState>> = RefCell::new(None);
}

// Global static storage for the shadow accumulator with Mutex for thread safety.
// This replaces thread-local storage because transactions in the same block
// may be processed on different threads.
use std::sync::Mutex;
use once_cell::sync::Lazy;
use std::collections::HashSet;

static SHADOW_ACCUMULATOR: Lazy<Mutex<Option<ShadowAccumulatorState>>> = Lazy::new(|| Mutex::new(None));

// Cache for l1_block_number from StartBlock internal transaction.
// This ensures deterministic l1_block_number across all execution passes (follower + validator).
// Uses a HashMap keyed by L2 block number to handle concurrent block execution.
use std::collections::HashMap;
static L1_BLOCK_NUMBER_CACHE: Lazy<Mutex<HashMap<u64, u64>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Set the cached l1_block_number for a given L2 block.
/// This should be called when processing the StartBlock internal transaction.
pub fn set_cached_l1_block_number(l2_block_number: u64, l1_block_number: u64) {
    let mut cache = L1_BLOCK_NUMBER_CACHE.lock().unwrap();
    cache.insert(l2_block_number, l1_block_number);
    
    // Keep cache size bounded to prevent memory leaks
    // Remove entries for blocks more than 100 blocks behind
    if l2_block_number > 100 {
        let min_block = l2_block_number - 100;
        cache.retain(|&k, _| k >= min_block);
    }
    
    tracing::debug!(
        target: "arb::l1_block_cache",
        l2_block_number = l2_block_number,
        l1_block_number = l1_block_number,
        cache_size = cache.len(),
        "Cached l1_block_number for block"
    );
}

/// Get the cached l1_block_number for a given L2 block.
/// Returns None if no cache exists for this block.
pub fn get_cached_l1_block_number(l2_block_number: u64) -> Option<u64> {
    let cache = L1_BLOCK_NUMBER_CACHE.lock().unwrap();
    cache.get(&l2_block_number).copied()
}

/// Clear the l1_block_number cache.
pub fn clear_l1_block_number_cache() {
    let mut cache = L1_BLOCK_NUMBER_CACHE.lock().unwrap();
    cache.clear();
}

// Track which (block_number, tx_hash) pairs have already been applied to prevent duplicate state application.
// This is needed because the same transaction may be executed in multiple "committing" contexts
// (e.g., building and validating), and we must only apply state changes once.
// CRITICAL: We dedupe by tx_hash, NOT by leaf_num, because different execution contexts may compute
// different leaf_nums for the same conceptual send (due to shadow accumulator contamination).
static APPLIED_TXS: Lazy<Mutex<(u64, HashSet<B256>)>> = Lazy::new(|| Mutex::new((0, HashSet::new())));

/// Store ArbSys state changes for later application.
pub fn store_arbsys_state(state: ArbSysMerkleState) {
    ARBSYS_STATE.with(|cell| {
        *cell.borrow_mut() = Some(state);
    });
}

/// Take the stored ArbSys state changes (clears the storage).
pub fn take_arbsys_state() -> Option<ArbSysMerkleState> {
    ARBSYS_STATE.with(|cell| {
        cell.borrow_mut().take()
    })
}

/// Clear any stored ArbSys state changes.
pub fn clear_arbsys_state() {
    ARBSYS_STATE.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// Check if a tx has already been applied for this block, and mark it as applied if not.
/// Returns true if this is the first time we're seeing this tx (should apply state).
/// Returns false if this tx was already applied (should skip).
/// CRITICAL: We dedupe by tx_hash (send_hash), NOT by leaf_num, because different execution
/// contexts may compute different leaf_nums for the same conceptual send.
pub fn try_mark_tx_applied(block_number: u64, send_hash: B256) -> bool {
    let mut applied = APPLIED_TXS.lock().unwrap();
    
    // If block number changed, reset the set
    if applied.0 != block_number {
        applied.0 = block_number;
        applied.1.clear();
    }
    
    // Try to insert the send_hash. Returns true if it was newly inserted.
    let is_new = applied.1.insert(send_hash);
    
    tracing::debug!(
        target: "arb::applied_txs",
        block_number = block_number,
        send_hash = ?send_hash,
        is_new = is_new,
        set_size = applied.1.len(),
        "Checking if tx was already applied"
    );
    
    is_new
}

/// Update the shadow accumulator with the new state after a transaction commits.
/// This should be called after applying deferred state changes.
pub fn update_shadow_accumulator(block_number: u64, new_size: u64, partials: &[(u64, B256)]) {
    let thread_id = std::thread::current().id();
    let mut shadow = SHADOW_ACCUMULATOR.lock().unwrap();
    let old_state = shadow.as_ref().map(|s| (s.block_number, s.size));
    
    // If block number changed, reset the shadow
    if shadow.as_ref().map(|s| s.block_number) != Some(block_number) {
        *shadow = Some(ShadowAccumulatorState {
            size: new_size,
            partials: Vec::new(),
            block_number,
        });
    }
    
    if let Some(ref mut s) = *shadow {
        s.size = new_size;
        // Update partials
        for (level, hash) in partials {
            let level = *level as usize;
            // Extend partials vector if needed
            while s.partials.len() <= level {
                s.partials.push(B256::ZERO);
            }
            s.partials[level] = *hash;
        }
    }
    
    tracing::debug!(
        target: "arb::shadow",
        ?thread_id,
        block_number = block_number,
        new_size = new_size,
        ?old_state,
        "SHADOW_UPDATE: updated shadow accumulator"
    );
}

/// Get the current size from the shadow accumulator, if available for this block.
pub fn get_shadow_size(block_number: u64) -> Option<u64> {
    let thread_id = std::thread::current().id();
    let shadow = SHADOW_ACCUMULATOR.lock().unwrap();
    let shadow_state = shadow.as_ref().map(|s| (s.block_number, s.size));
    let result = shadow.as_ref().and_then(|s| {
        if s.block_number == block_number {
            Some(s.size)
        } else {
            None
        }
    });
    
    tracing::debug!(
        target: "arb::shadow",
        ?thread_id,
        requested_block = block_number,
        ?shadow_state,
        ?result,
        "SHADOW_GET: checking shadow accumulator"
    );
    
    result
}

/// Get a partial hash from the shadow accumulator, if available for this block.
pub fn get_shadow_partial(block_number: u64, level: u64) -> Option<B256> {
    let shadow = SHADOW_ACCUMULATOR.lock().unwrap();
    shadow.as_ref().and_then(|s| {
        if s.block_number == block_number {
            s.partials.get(level as usize).copied()
        } else {
            None
        }
    })
}

/// Clear the shadow accumulator (call at block boundaries or when starting a new replay).
/// NOTE: Based on instrumentation, this is called very frequently (per-tx or per-pass),
/// so we should NOT call this in apply_pre_execution_changes. Instead, the shadow
/// auto-resets when the block number changes in update_shadow_accumulator.
pub fn clear_shadow_accumulator() {
    let thread_id = std::thread::current().id();
    let mut shadow = SHADOW_ACCUMULATOR.lock().unwrap();
    let old_state = shadow.as_ref().map(|s| (s.block_number, s.size));
    *shadow = None;
    
    tracing::debug!(
        target: "arb::shadow",
        ?thread_id,
        ?old_state,
        "SHADOW_CLEAR: cleared shadow accumulator"
    );
}

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

/// ArbOSVersion function selector: keccak256("arbOSVersion()")[:4] = 0x051038f2
const ARBOS_VERSION_SELECTOR: [u8; 4] = [0x05, 0x10, 0x38, 0xf2];

/// ArbBlockNumber function selector: keccak256("arbBlockNumber()")[:4] = 0xa3b1b31d
const ARB_BLOCK_NUMBER_SELECTOR: [u8; 4] = [0xa3, 0xb1, 0xb3, 0x1d];

/// L2ToL1Tx event topic:keccak256("L2ToL1Tx(address,address,uint256,uint256,uint256,uint256,uint256,uint256,bytes)")
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
    // Use new_stateful() to mark this precompile as non-pure.
    // This prevents the validator from wrapping it in CachedPrecompile,
    // which would cache results and drop logs.
    DynPrecompile::new_stateful(PrecompileId::custom("arbsys"), arbsys_precompile_handler)
}

/// Main precompile handler function.
fn arbsys_precompile_handler(mut input: PrecompileInput<'_>) -> PrecompileResult {
    let exec_id = EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
    let data = input.data;
    
    // Check minimum input length for selector
    if data.len() < 4 {
        return Err(PrecompileError::other("input too short"));
    }
    
    let selector: [u8; 4] = [data[0], data[1], data[2], data[3]];
    
    // Log execution pass info
    tracing::trace!(
        target: "arb::arbsys_precompile",
        exec_id = exec_id,
        selector = ?selector,
        caller = ?input.caller,
        value = ?input.value,
        "EXEC_COUNTER: ArbSys precompile handler called"
    );
    
    match selector {
        SEND_TX_TO_L1_SELECTOR => {
            handle_send_tx_to_l1(&mut input, exec_id)
        }
        WITHDRAW_ETH_SELECTOR => {
            handle_withdraw_eth(&mut input, exec_id)
        }
        ARBOS_VERSION_SELECTOR => {
            handle_arbos_version(&mut input, exec_id)
        }
        ARB_BLOCK_NUMBER_SELECTOR => {
            handle_arb_block_number(&mut input, exec_id)
        }
        _ => {
            // Unknown selector - revert
            Err(PrecompileError::other("unknown selector"))
        }
    }
}

/// Handle SendTxToL1 call.
/// Input format: selector (4 bytes) + destination (32 bytes) + offset (32 bytes) + length (32 bytes) + data
fn handle_send_tx_to_l1(input: &mut PrecompileInput<'_>, exec_id: u64) -> PrecompileResult {
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
    
    execute_send_tx_to_l1(input, destination, calldata_for_l1, exec_id)
}

/// Handle WithdrawEth call.
/// Input format: selector (4 bytes) + destination (32 bytes)
fn handle_withdraw_eth(input: &mut PrecompileInput<'_>, exec_id: u64) -> PrecompileResult {
    let data = input.data;
    
    // Parse destination address
    if data.len() < 4 + 32 {
        return Err(PrecompileError::other("input too short for destination"));
    }
    let mut dest_bytes = [0u8; 20];
    dest_bytes.copy_from_slice(&data[4 + 12..4 + 32]);
    let destination = Address::from(dest_bytes);
    
    // WithdrawEth is just SendTxToL1 with empty calldata
    execute_send_tx_to_l1(input, destination, Bytes::new(), exec_id)
}

/// Handle ArbOSVersion call.
/// Returns the current ArbOS version as a uint256.
/// Go nitro returns: 55 + state.ArbOSVersion() (Nitro starts at version 56)
fn handle_arbos_version(input: &mut PrecompileInput<'_>, exec_id: u64) -> PrecompileResult {
    let gas_limit = input.gas;
    let internals = input.internals_mut();
    
    // Load the ArbOS state account first to ensure it's in the journal
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed for ArbOS state: {:?}", e)))?;
    
    // Read ArbOS version from storage at offset 0 in ArbOS state
    // Go nitro stores the version at versionOffset = 0
    // The slot is calculated using storage_key_map with empty storage key (root storage)
    let arbos_version = {
        // Go nitro uses storage_key_map with empty slice for root storage
        // storage_key_map([], 0) = keccak256([] || key_bytes[0:31]) with key_bytes[31] preserved
        let slot = compute_arbos_version_slot();
        let value = internals.sload(ARBOS_STATE_ADDRESS, slot)
            .map_err(|_| PrecompileError::other("failed to read ArbOS version"))?;
        value.data.as_limbs()[0] // Get the u64 value
    };
    
    // Go nitro returns: 55 + arbosVersion (Nitro starts at version 56)
    let version = 55u64 + arbos_version;
    
    tracing::debug!(
        target: "arb::arbsys_precompile",
        exec_id = exec_id,
        arbos_version = arbos_version,
        returned_version = version,
        "ArbOSVersion called"
    );
    
    // Return version as uint256 (32 bytes, big-endian)
    let mut output = [0u8; 32];
    output[24..32].copy_from_slice(&version.to_be_bytes());
    
    // Gas cost: 800 (SloadGasEIP2200 for reading version) + 3 (CopyGas for 32-byte output)
    // Go nitro charges this via OpenArbosState (800) and resultCost (3) in precompile.go
    const SLOAD_GAS_EIP2200: u64 = 800;
    const COPY_GAS: u64 = 3; // params.CopyGas in Go
    let gas_cost = (SLOAD_GAS_EIP2200 + COPY_GAS).min(gas_limit);

    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle ArbBlockNumber call.
/// Returns the current L2 block number as a uint256.
/// Go nitro implementation: return evm.Context.BlockNumber, nil
fn handle_arb_block_number(input: &mut PrecompileInput<'_>, exec_id: u64) -> PrecompileResult {
    let gas_limit = input.gas;
    let internals = input.internals_mut();
    
    // Get the current block number from the EVM context
    let block_number = internals.block_number();
    
    tracing::debug!(
        target: "arb::arbsys_precompile",
        exec_id = exec_id,
        block_number = ?block_number,
        "ArbBlockNumber called"
    );
    
    // Return block number as uint256 (32 bytes, big-endian)
    let output: [u8; 32] = block_number.to_be_bytes();
    
    // Gas cost: just the base precompile cost (3 for CopyGas)
    // Go nitro doesn't charge any storage reads for this function
    const COPY_GAS: u64 = 3;
    let gas_cost = COPY_GAS.min(gas_limit);

    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Execute the SendTxToL1 logic.
fn execute_send_tx_to_l1(
    input: &mut PrecompileInput<'_>,
    destination: Address,
    calldata_for_l1: Bytes,
    exec_id: u64,
) -> PrecompileResult {
    let caller = input.caller;
    let value = input.value;
    let gas_limit = input.gas;  // Capture gas limit before consuming input
    let internals = input.internals_mut();
    
    // Get block info
    let block_number = internals.block_number();
    let block_timestamp = internals.block_timestamp();
    let block_num_u64: u64 = block_number.try_into().unwrap_or(0);
    
    // Get L1 block number from cache or ArbOS state
    // The cache is set by StartBlock internal tx to ensure determinism across execution passes
    let l1_block_number = get_l1_block_number(internals, block_num_u64)?;
    
    tracing::debug!(
        target: "arb::arbsys_precompile",
        exec_id = exec_id,
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
    
    // Append to Merkle accumulator and get events + state changes to apply later
    // Pass block_number so the shadow accumulator can track state across txs in the same block
    let (leaf_num, merkle_events, new_size, partials_to_set) = append_to_merkle_accumulator(internals, send_hash, block_num_u64)?;
    
    tracing::debug!(
        target: "arb::arbsys_precompile",
        leaf_num = leaf_num,
        send_hash = ?send_hash,
        merkle_events_count = merkle_events.len(),
        new_size = new_size,
        partials_count = partials_to_set.len(),
        "Merkle accumulator computed, storing state changes for deferred application"
    );
    
    // Store the state changes in the thread-local sink for later application
    // build.rs will apply these after EVM execution using storage.rs
    store_arbsys_state(ArbSysMerkleState {
        new_size,
        partials: partials_to_set,
        send_hash,
        leaf_num,
        value_to_burn: value,
        block_number: block_num_u64,
    });
    
    // NOTE: We no longer burn the value here because the EVM journal might not persist.
    // The value burning will be handled by build.rs after EVM execution.
    // The EVM has already transferred the value from caller to ArbSys (0x64),
    // so the balance is correct for the precompile's execution.
    
    // Emit SendMerkleUpdate events
    // Go nitro ABI: event SendMerkleUpdate(uint256 indexed reserved, bytes32 indexed hash, uint256 indexed position)
    // All 3 parameters are indexed, so they go in topics, not data
    let send_merkle_update_topic = send_merkle_update_topic();
    for event in &merkle_events {
        // Position is encoded as (level << 192) + numLeaves (matches Go nitro's LevelAndLeaf.ToBigInt())
        let position = level_and_leaf_to_u256(event.level, event.num_leaves);
        
        // All parameters are indexed (in topics), data is empty
        // topic[0] = event signature
        // topic[1] = reserved (always 0)
        // topic[2] = hash (bytes32)
        // topic[3] = position (uint256 encoded as (level << 192) + numLeaves)
        let log = Log::new(
            ARBSYS_ADDRESS,
            vec![
                send_merkle_update_topic,
                B256::ZERO,  // reserved = 0
                event.hash,  // hash (already B256)
                B256::from(position),  // position as B256
            ],
            Bytes::new(),  // empty data - all params are indexed
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
    // Pass current_size (before increment) to determine if a partial read happened
    let current_size = new_size - 1;
    let calculated_gas = calculate_gas_cost(calldata_for_l1.len(), merkle_events.len(), current_size);
    // Cap gas_used to available gas to prevent underflow panic in alloy-evm
    // Arbitrum's gas model allows precompiles to charge from the transaction burner,
    // but alloy-evm expects precompile gas <= call gas
    let gas_used = calculated_gas.min(gas_limit);

    Ok(PrecompileOutput::new(gas_used, output.into()))
}

/// Get L1 block number from cache or ArbOS state.
/// 
/// This function first checks the l1_block_number cache (set by StartBlock internal tx),
/// and falls back to reading from ArbOS storage if no cache exists.
/// Using the cache ensures deterministic l1_block_number across all execution passes.
fn get_l1_block_number(internals: &mut alloy_evm::EvmInternals<'_>, l2_block_number: u64) -> Result<u64, PrecompileError> {
    // First, check the cache for this L2 block
    // The cache MUST be populated by StartBlock internal tx before any ArbSys calls
    if let Some(cached_l1_block) = get_cached_l1_block_number(l2_block_number) {
        tracing::debug!(
            target: "arb::l1_block_cache",
            l2_block_number = l2_block_number,
            cached_l1_block = cached_l1_block,
            "Using cached l1_block_number"
        );
        return Ok(cached_l1_block);
    }
    
    // Cache miss - this should not happen in normal operation because StartBlock
    // should always be processed before any user transactions that call ArbSys.
    // However, we try to fall back to reading from storage for robustness.
    tracing::warn!(
        target: "arb::l1_block_cache",
        l2_block_number = l2_block_number,
        "Cache miss for l1_block_number - falling back to storage read"
    );
    
    // Fall back to reading from storage
    // Storage layout in Go nitro:
    // - ArbOS state is at ARBOS_STATE_ADDRESS
    // - blockhashes substorage is at subspace ID 6 (blockhashesSubspace = []byte{6})
    // - l1BlockNumber is at offset 0 within blockhashes substorage
    
    // Try to load the account into the journal before accessing storage
    // If this fails, return an error instead of panicking
    match internals.load_account(ARBOS_STATE_ADDRESS) {
        Ok(_) => {}
        Err(e) => {
            tracing::error!(
                target: "arb::l1_block_cache",
                l2_block_number = l2_block_number,
                error = ?e,
                "Failed to load ArbOS account for l1_block_number read"
            );
            return Err(PrecompileError::other(format!(
                "l1_block_number cache miss and load_account failed for block {}: {:?}",
                l2_block_number, e
            )));
        }
    }
    
    // Compute the storage slot for blockhashes.l1BlockNumber
    // This matches Go nitro's storage.OpenSubStorage and storage.NewSlot
    // blockhashesSubspace = []byte{6} in Go nitro
    let blockhashes_key = compute_substorage_key(&[], 6);
    let l1_block_slot = compute_storage_slot(&blockhashes_key, 0);
    
    // Try to read from storage, return error if it fails
    match internals.sload(ARBOS_STATE_ADDRESS, l1_block_slot) {
        Ok(result) => {
            let value: u64 = result.data.try_into().unwrap_or(0);
            tracing::debug!(
                target: "arb::l1_block_cache",
                l2_block_number = l2_block_number,
                l1_block_from_storage = value,
                "Read l1_block_number from storage (cache miss)"
            );
            Ok(value)
        }
        Err(e) => {
            tracing::error!(
                target: "arb::l1_block_cache",
                l2_block_number = l2_block_number,
                error = ?e,
                "Failed to sload l1_block_number from storage"
            );
            Err(PrecompileError::other(format!(
                "l1_block_number cache miss and sload failed for block {}: {:?}",
                l2_block_number, e
            )))
        }
    }
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
/// Append to Merkle accumulator and return (leaf_num, events, partials_to_set).
/// 
/// IMPORTANT: This function now READS from the EVM state but does NOT WRITE to it.
/// Instead, it returns the list of partial hashes that need to be set.
/// The caller is responsible for storing these in the thread-local sink,
/// and build.rs will apply them after EVM execution using storage.rs.
///
/// The `block_number` parameter is used to check the shadow accumulator for
/// state from previous transactions in the same block.
fn append_to_merkle_accumulator(
    internals: &mut alloy_evm::EvmInternals<'_>,
    item_hash: B256,
    block_number: u64,
) -> Result<(u64, Vec<MerkleTreeNodeEvent>, u64, Vec<(u64, B256)>), PrecompileError> {
    // Storage layout for SendMerkleAccumulator:
    // - Substorage at subspace ID 5 (sendMerkleSubspace = []byte{5})
    // - Size at offset 0 within substorage
    // - Partials at offset 2+ within substorage
    
    // IMPORTANT: Must load the account into the journal before accessing storage.
    // This is required by revm's journal semantics - sload will panic if the account
    // isn't loaded first.
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed in merkle accumulator: {:?}", e)))?;
    
    // sendMerkleSubspace = []byte{5} in Go nitro
    let merkle_key = compute_substorage_key(&[], 5);
    let size_slot = compute_storage_slot(&merkle_key, 0);
    
    // IMPORTANT: Always read from EVM state, NOT from the global shadow accumulator.
    // The global shadow accumulator causes cross-context contamination when the same
    // transaction is executed in multiple contexts (non-canonical and canonical passes).
    // Each execution context should compute its own leaf_num from the database state.
    // For blocks with multiple ArbSys calls, the deferred state application ensures
    // that tx2 sees tx1's updated state within the same execution context.
    let current_size_result = internals.sload(ARBOS_STATE_ADDRESS, size_slot)
        .map_err(|e| PrecompileError::other(format!("sload size failed: {:?}", e)))?;
    let current_size: u64 = current_size_result.data.try_into().unwrap_or(0);
    
    tracing::debug!(
        target: "arb::arbsys_precompile",
        current_size = current_size,
        block_number = block_number,
        "Merkle accumulator: read size from EVM state (no shadow)"
    );
    
    let new_size = current_size + 1;
    
    tracing::debug!(
        target: "arb::arbsys_precompile",
        current_size = current_size,
        new_size = new_size,
        size_slot = ?size_slot,
        block_number = block_number,
        "Merkle accumulator: read current size, will defer writes"
    );
    
    // Collect partial hashes to set (instead of calling sstore directly)
    let mut partials_to_set: Vec<(u64, B256)> = Vec::new();
    let mut events = Vec::new();
    let mut level = 0u64;
    let mut so_far = keccak256(item_hash.as_slice());
    
    loop {
        if level == calc_num_partials(current_size) {
            // Set this level's partial
            partials_to_set.push((level, so_far));
            break;
        }
        
        // Always read from EVM state, NOT from the global shadow accumulator.
        // See comment above about cross-context contamination.
        let this_level = get_partial(internals, &merkle_key, level)?;
        
        if this_level == B256::ZERO {
            // Set this level's partial
            partials_to_set.push((level, so_far));
            break;
        }
        
        // Combine hashes
        let mut combined = Vec::with_capacity(64);
        combined.extend_from_slice(this_level.as_slice());
        combined.extend_from_slice(so_far.as_slice());
        so_far = keccak256(&combined);
        
        // Clear this level (set to zero)
        partials_to_set.push((level, B256::ZERO));
        
        level += 1;
        
        // Event is emitted AFTER incrementing level
        events.push(MerkleTreeNodeEvent {
            level,
            num_leaves: new_size - 1,
            hash: so_far,
        });
    }
    
    let leaf_num = new_size - 1;
    Ok((leaf_num, events, new_size, partials_to_set))
}

/// Calculate number of partials for a given size.
/// This matches Go nitro's CalcNumPartials which uses Log2ceil.
/// Log2ceil(value) = 64 - leading_zeros(value)
fn calc_num_partials(size: u64) -> u64 {
    if size == 0 {
        return 0;
    }
    // Go nitro: return uint64(64 - bits.LeadingZeros64(value))
    64 - size.leading_zeros() as u64
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
    
    tracing::debug!(
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

/// Compute the storage slot for ArbOS version (offset 0 in root storage).
/// Go nitro uses empty slice for root storage, so this is compute_storage_slot(&[], 0).
fn compute_arbos_version_slot() -> U256 {
    // versionOffset = 0 in Go nitro
    compute_storage_slot(&[], 0)
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
/// 
/// The `current_size` parameter is the Merkle accumulator size BEFORE the append.
/// This is used to determine if a partial read happens:
/// - For size 0→1: NO partial read (we exit immediately because level == calc_num_partials(0) == 0)
/// - For size N→N+1 (N > 0): 1 partial read (we read partial[0] before deciding to exit or continue)
fn calculate_gas_cost(calldata_for_l1_len: usize, merkle_events_count: usize, current_size: u64) -> u64 {
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
    // Each level involves: 1 sstore to clear the partial, then keccak to combine hashes
    // NOTE: The partial read is already counted in final_partial_read_cost below,
    // so we only count the sstore to clear and the keccak here.
    let merkle_level_cost = if merkle_events_count > 0 {
        // Each event means we combined two hashes and wrote to a higher level
        // sstore to clear partial, keccak to combine (partial read is counted separately)
        let per_level_cost = STORAGE_WRITE_ZERO_COST + (30 + 6 * 2);  // 2 words for 64 bytes
        (merkle_events_count as u64) * per_level_cost
    } else {
        0
    };
    
    // Final partial write (always happens) - this includes a read cost
    // Go nitro always charges for the final partial read/write even for size 0→1
    let final_partial_read_cost = STORAGE_READ_COST;
    let final_partial_write_cost = STORAGE_WRITE_COST;
    
    // Partial read cost for getPartial calls in the Append loop.
    // 
    // Go nitro's Append loop calls getPartial() for each level it processes:
    // - For size 0→1: 0 reads (exit immediately via level == CalcNumPartials(0) == 0)
    // - For size N→N+1: reads = merkle_events_count + termination_read
    //   where termination_read = 0 if new_size is power of 2 (exit via full carry)
    //                          = 1 if new_size is NOT power of 2 (exit via empty partial)
    //
    // Examples:
    // - size 0→1: 0 reads (current_size == 0)
    // - size 1→2: 1 read (merkle_events_count=1, new_size=2 is power of 2, no termination read)
    // - size 3→4: 2 reads (merkle_events_count=2, new_size=4 is power of 2, no termination read)
    // - size 4→5: 1 read (merkle_events_count=0, new_size=5 is NOT power of 2, 1 termination read)
    // - size 5→6: 2 reads (merkle_events_count=1, new_size=6 is NOT power of 2, 1 termination read)
    let additional_partial_read_cost = if current_size > 0 {
        let new_size = current_size + 1;
        let is_power_of_two = new_size > 0 && (new_size & (new_size - 1)) == 0;
        let termination_read = if is_power_of_two { 0u64 } else { 1u64 };
        let total_reads = merkle_events_count as u64 + termination_read;
        total_reads * STORAGE_READ_COST
    } else {
        0
    };
    
    // Initial hash of item (keccak of sendHash)
    // NOTE: Go nitro uses crypto.Keccak256 directly (not acc.Keccak) for the initial hash,
    // so it does NOT charge gas for this operation. See merkleAccumulator.go line 134.
    let initial_hash_cost = 0u64;
    
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
        + additional_partial_read_cost
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
        // calc_num_partials uses Log2ceil: 64 - leading_zeros(size)
        assert_eq!(calc_num_partials(0), 0);
        assert_eq!(calc_num_partials(1), 1);  // 64 - 63 = 1
        assert_eq!(calc_num_partials(2), 2);  // 64 - 62 = 2
        assert_eq!(calc_num_partials(3), 2);  // 64 - 62 = 2
        assert_eq!(calc_num_partials(4), 3);  // 64 - 61 = 3
        assert_eq!(calc_num_partials(5), 3);  // 64 - 61 = 3
    }
}
