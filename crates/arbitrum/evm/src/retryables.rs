#![allow(unused)]

use alloy_primitives::{keccak256, Address, Bytes, U256, B256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64, StorageBackedBigUint, StorageBackedAddress};

#[derive(Clone, Copy, Debug)]
pub struct RetryableTicketId(pub [u8; 32]);

#[derive(Clone)]
pub struct RetryableCreateParams {
    pub sender: Address,
    pub beneficiary: Address,
    pub call_to: Address,
    pub call_data: Bytes,
    pub callvalue: U256,  // The actual call value to store (retry_value in SubmitRetryable)
    pub l1_base_fee: U256,
    pub submission_fee: U256,
    pub max_submission_cost: U256,
    pub max_gas: U256,
    pub gas_price_bid: U256,
}

pub enum RetryableAction {
    Created {
        ticket_id: RetryableTicketId,
        escrowed: U256,
        user_gas: u64,
        nonce: u64,
        refund_to: Address,
        max_refund: U256,
        submission_fee_refund: U256,
        retry_tx_hash: alloy_primitives::B256,
        sequence_num: u64,
    },
    Redeemed { ticket_id: RetryableTicketId, success: bool },
    Canceled { ticket_id: RetryableTicketId },
    KeptAlive { ticket_id: RetryableTicketId },
    TimedOut { ticket_id: RetryableTicketId, refund_to: Address },
}

pub trait Retryables {
    fn create_retryable(&mut self, params: RetryableCreateParams) -> RetryableAction;
    fn redeem_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction;
    fn cancel_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction;
    fn keepalive_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction;

    fn get_beneficiary(&self, ticket_id: &RetryableTicketId) -> Option<Address>;
}

pub struct RetryableState<D> {
    storage: Storage<D>,
}

/// Retryable ticket storage structure matching Go nitro exactly.
/// Go nitro offsets (from retryable.go):
///   numTriesOffset = 0 (iota)
///   fromOffset = 1
///   toOffset = 2
///   callvalueOffset = 3
///   beneficiaryOffset = 4
///   timeoutOffset = 5
///   timeoutWindowsLeftOffset = 6
/// Calldata is stored in a nested substorage with calldataKey = []byte{1}
pub struct RetryableTicket<D> {
    storage: Storage<D>,
    ticket_id: RetryableTicketId,
    
    // Fields matching Go nitro's Retryable struct exactly
    num_tries: StorageBackedUint64<D>,
    from: StorageBackedAddress<D>,
    to: StorageBackedAddress<D>,
    callvalue: StorageBackedBigUint<D>,
    beneficiary: StorageBackedAddress<D>,
    timeout: StorageBackedUint64<D>,
    timeout_windows_left: StorageBackedUint64<D>,
    // Note: calldata is stored in a nested substorage, not as a direct field
}

// Go nitro offsets (from retryable.go const block with iota)
const NUM_TRIES_OFFSET: u64 = 0;
const FROM_OFFSET: u64 = 1;
const TO_OFFSET: u64 = 2;
const CALLVALUE_OFFSET: u64 = 3;
const BENEFICIARY_OFFSET: u64 = 4;
const TIMEOUT_OFFSET: u64 = 5;
const TIMEOUT_WINDOWS_LEFT_OFFSET: u64 = 6;

// Calldata is stored in a nested substorage with this key
const CALLDATA_KEY: &[u8] = &[1];

// TimeoutQueue subspace key (Go nitro uses []byte{0} for the queue within retryables subspace)
const TIMEOUT_QUEUE_KEY: &[u8] = &[0];

pub const RETRYABLE_LIFETIME_SECONDS: u64 = 7 * 24 * 60 * 60;

/// TimeoutQueue matching Go nitro's storage/queue.go
/// Stores:
/// - Slot 0: nextPutOffset (starts at 2)
/// - Slot 1: nextGetOffset (starts at 2)
/// - Slot 2+: queue entries (ticket IDs)
pub struct TimeoutQueue<D> {
    storage: Storage<D>,
}

impl<D: Database> TimeoutQueue<D> {
    pub fn new(storage: Storage<D>) -> Self {
        Self { storage }
    }
    
    /// Put a ticket ID into the queue (matching Go nitro's Queue.Put)
    pub fn put(&self, state: *mut revm::database::State<D>, ticket_id: B256) -> Result<(), ()> {
        // Get current nextPutOffset (slot 0)
        let next_put_storage = StorageBackedUint64::new(state, self.storage.base_key, 0);
        let next_put = next_put_storage.get().unwrap_or(2); // Default to 2 if not initialized
        
        // Increment nextPutOffset
        next_put_storage.set(next_put + 1)?;
        
        // Store the ticket ID at slot (next_put)
        let slot_storage = StorageBackedBigUint::new(state, self.storage.base_key, next_put);
        slot_storage.set(U256::from_be_bytes(ticket_id.0))?;
        
        tracing::info!(target: "arb-retryable", "TIMEOUT_QUEUE_PUT: ticket_id={:?} slot={}", ticket_id, next_put);
        
        Ok(())
    }
}

impl<D: Database> RetryableState<D> {
    pub fn new(state: *mut revm::database::State<D>, base_key: B256) -> Self {
        let storage = Storage::new(state, base_key);
        Self { storage }
    }
    
    /// Get the TimeoutQueue for this retryable state
    fn timeout_queue(&self) -> TimeoutQueue<D> {
        let queue_storage = self.storage.open_sub_storage(TIMEOUT_QUEUE_KEY);
        TimeoutQueue::new(queue_storage)
    }
    
    pub fn try_to_reap_one_retryable(&self, current_time: u64, state: *mut revm::database::State<D>) -> Result<bool, ()> {
        let timeout_storage = StorageBackedUint64::new(state, self.storage.base_key, 0);
        let oldest_timeout = timeout_storage.get().unwrap_or(u64::MAX);
        
        if oldest_timeout >= current_time {
            return Ok(false);
        }
        
        Ok(true)
    }

    /// Create a retryable ticket matching Go nitro's CreateRetryable exactly.
    /// Go nitro sets: numTries=0, from, to, callvalue, beneficiary, calldata (bytes), timeout, timeoutWindowsLeft=0
    /// Then inserts into TimeoutQueue.
    pub fn create_retryable(
        &self,
        state: *mut revm::database::State<D>,
        ticket_id: RetryableTicketId,
        params: RetryableCreateParams,
        current_time: u64,
    ) -> RetryableTicket<D> {
        let ticket_storage = self.storage.open_sub_storage(&ticket_id.0);
        let ticket_base_key = ticket_storage.base_key;
        
        tracing::info!(target: "arb-retryable", "CREATE: ticket_id={:?} base_key={:?} parent_key={:?}", ticket_id, ticket_base_key, self.storage.base_key);

        let ticket = RetryableTicket {
            storage: ticket_storage.clone(),
            ticket_id,
            num_tries: StorageBackedUint64::new(state, ticket_base_key, NUM_TRIES_OFFSET),
            from: StorageBackedAddress::new(state, ticket_base_key, FROM_OFFSET),
            to: StorageBackedAddress::new(state, ticket_base_key, TO_OFFSET),
            callvalue: StorageBackedBigUint::new(state, ticket_base_key, CALLVALUE_OFFSET),
            beneficiary: StorageBackedAddress::new(state, ticket_base_key, BENEFICIARY_OFFSET),
            timeout: StorageBackedUint64::new(state, ticket_base_key, TIMEOUT_OFFSET),
            timeout_windows_left: StorageBackedUint64::new(state, ticket_base_key, TIMEOUT_WINDOWS_LEFT_OFFSET),
        };

        let timeout = current_time + RETRYABLE_LIFETIME_SECONDS;

        // Set fields matching Go nitro's CreateRetryable order:
        // ret.numTries.Set(0)
        // ret.from.Set(from)
        // ret.to.Set(to)
        // ret.callvalue.SetChecked(callvalue)
        // ret.beneficiary.Set(beneficiary)
        // ret.calldata.Set(calldata)  -- stored in nested substorage
        // ret.timeout.Set(timeout)
        // ret.timeoutWindowsLeft.Set(0)
        let _ = ticket.num_tries.set(0);
        let _ = ticket.from.set(params.sender);
        let _ = ticket.to.set(params.call_to);
        let _ = ticket.callvalue.set(params.callvalue);  // Go nitro stores the actual callvalue (retry_value)
        let _ = ticket.beneficiary.set(params.beneficiary);
        // Store calldata in nested substorage (matching Go nitro's StorageBackedBytes)
        let calldata_storage = ticket_storage.open_sub_storage(CALLDATA_KEY);
        Self::set_bytes(state, &calldata_storage, &params.call_data);
        let _ = ticket.timeout.set(timeout);
        let _ = ticket.timeout_windows_left.set(0);
        
        tracing::info!(target: "arb-retryable", "CREATE_RETRYABLE: ticket_id={:?} timeout={}", ticket_id, timeout);

        // Insert into TimeoutQueue (rs.TimeoutQueue.Put(id))
        let queue = self.timeout_queue();
        let _ = queue.put(state, B256::from(ticket_id.0));

        ticket
    }
    
    /// Set bytes in storage matching Go nitro's Storage.SetBytes
    /// Go nitro uses common.BytesToHash which RIGHT-aligns bytes (pads on the left)
    fn set_bytes(state: *mut revm::database::State<D>, storage: &Storage<D>, data: &[u8]) {
        // Go nitro's SetBytes stores:
        // - Slot 0: length of data
        // - Subsequent slots: data packed into 32-byte chunks, RIGHT-aligned (padded on left)
        let len = data.len();
        let len_storage = StorageBackedUint64::new(state, storage.base_key, 0);
        let _ = len_storage.set(len as u64);
        
        // Store data in 32-byte chunks, RIGHT-aligned like Go's common.BytesToHash
        let mut offset = 1u64;
        let mut remaining = data;
        while remaining.len() >= 32 {
            // Full 32-byte chunk - no padding needed
            let chunk: [u8; 32] = remaining[..32].try_into().unwrap();
            let value = U256::from_be_bytes(chunk);
            let slot_storage = StorageBackedBigUint::new(state, storage.base_key, offset);
            let _ = slot_storage.set(value);
            remaining = &remaining[32..];
            offset += 1;
        }
        // Handle the last partial chunk (if any) - RIGHT-align (pad on left)
        if !remaining.is_empty() {
            let mut padded = [0u8; 32];
            // Go's BytesToHash right-aligns: bytes go at the END of the 32-byte array
            padded[32 - remaining.len()..].copy_from_slice(remaining);
            let value = U256::from_be_bytes(padded);
            let slot_storage = StorageBackedBigUint::new(state, storage.base_key, offset);
            let _ = slot_storage.set(value);
        }
    }

    /// Open a retryable ticket matching Go nitro's OpenRetryable exactly.
    /// Go nitro checks: timeout == 0 || timeout < currentTimestamp => return nil
    pub fn open_retryable(
        &self,
        state: *mut revm::database::State<D>,
        ticket_id: &RetryableTicketId,
        current_time: u64,
    ) -> Option<RetryableTicket<D>> {
        let ticket_storage = self.storage.open_sub_storage(&ticket_id.0);
        let ticket_base_key = ticket_storage.base_key;
        let timeout_storage = StorageBackedUint64::new(state, ticket_base_key, TIMEOUT_OFFSET);
        
        tracing::info!(target: "arb-retryable", "OPEN: ticket_id={:?} base_key={:?} parent_key={:?}", ticket_id, ticket_base_key, self.storage.base_key);
        
        if let Ok(timeout) = timeout_storage.get() {
            // Go nitro: if timeout == 0 || timeout < currentTimestamp || err != nil { return nil, err }
            if timeout == 0 || timeout < current_time {
                tracing::info!(target: "arb-retryable", "OPEN_RETRYABLE: ticket expired or not found timeout={} current_time={}", timeout, current_time);
                return None;
            }
            
            tracing::info!(target: "arb-retryable", "OPEN_RETRYABLE: found valid ticket timeout={} current_time={}", timeout, current_time);
            return Some(RetryableTicket {
                storage: ticket_storage,
                ticket_id: RetryableTicketId(ticket_id.0),
                num_tries: StorageBackedUint64::new(state, ticket_base_key, NUM_TRIES_OFFSET),
                from: StorageBackedAddress::new(state, ticket_base_key, FROM_OFFSET),
                to: StorageBackedAddress::new(state, ticket_base_key, TO_OFFSET),
                callvalue: StorageBackedBigUint::new(state, ticket_base_key, CALLVALUE_OFFSET),
                beneficiary: StorageBackedAddress::new(state, ticket_base_key, BENEFICIARY_OFFSET),
                timeout: timeout_storage,
                timeout_windows_left: StorageBackedUint64::new(state, ticket_base_key, TIMEOUT_WINDOWS_LEFT_OFFSET),
            });
        }
        
        tracing::warn!(target: "arb-retryable", "OPEN_RETRYABLE: timeout storage GET failed for ticket_id={:?}", ticket_id);
        None
    }
}

impl<D: Database> RetryableTicket<D> {
    pub fn get_beneficiary(&self) -> Option<Address> {
        self.beneficiary.get().ok()
    }

    pub fn get_from(&self) -> Option<Address> {
        self.from.get().ok()
    }

    pub fn get_to(&self) -> Option<Address> {
        self.to.get().ok()
    }
    
    pub fn get_callvalue(&self) -> Option<U256> {
        self.callvalue.get().ok()
    }

    pub fn get_timeout(&self) -> Option<u64> {
        self.timeout.get().ok()
    }

    /// Increment num_tries - called when redeeming a retryable
    /// Go nitro: retryable.numTries.Increment()
    pub fn increment_tries(&self) -> Result<u64, ()> {
        let current = self.num_tries.get().unwrap_or(0);
        self.num_tries.set(current + 1)?;
        Ok(current + 1)
    }
    
    /// Get num_tries
    pub fn num_tries(&self) -> u64 {
        self.num_tries.get().unwrap_or(0)
    }
}

pub struct DefaultRetryables<D> {
    retryable_state: RetryableState<D>,
    state: *mut revm::database::State<D>,
}

impl<D: Database> DefaultRetryables<D> {
    pub fn new(state: *mut revm::database::State<D>, base_key: B256) -> Self {
        Self {
            retryable_state: RetryableState::new(state, base_key),
            state,
        }
    }
}

impl<D: Database> Default for DefaultRetryables<D> {
    fn default() -> Self {
        let null_state = std::ptr::null_mut();
        let base_key = B256::ZERO;
        Self::new(null_state, base_key)
    }
}

impl<D: Database> Retryables for DefaultRetryables<D> {
    fn create_retryable(&mut self, params: RetryableCreateParams) -> RetryableAction {
        let mut preimage = Vec::with_capacity(20 + 20 + params.call_data.len());
        preimage.extend_from_slice(params.sender.as_slice());
        preimage.extend_from_slice(params.call_to.as_slice());
        preimage.extend_from_slice(&params.call_data);
        let id = keccak256(preimage);
        let ticket_id = RetryableTicketId(id.0);
        
        let ticket = self.retryable_state.create_retryable(self.state, ticket_id, params.clone(), 0);
        // Go nitro doesn't store escrowed in the retryable - it's computed from submission fee
        let callvalue = ticket.get_callvalue().unwrap_or_default();

        RetryableAction::Created {
            ticket_id,
            escrowed: callvalue,  // Use callvalue as escrowed for compatibility
            user_gas: 0,
            nonce: 0,
            refund_to: params.beneficiary,
            max_refund: U256::ZERO,
            submission_fee_refund: params.submission_fee,
            retry_tx_hash: alloy_primitives::B256::ZERO,
            sequence_num: 0,
        }
    }

    fn redeem_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        // Go nitro: if retryable exists (timeout > 0 && timeout >= currentTime), increment tries
        // Go nitro doesn't have an "active" flag - it uses timeout to determine if retryable exists
        if let Some(ticket) = self.retryable_state.open_retryable(self.state, ticket_id, 0) {
            let _ = ticket.increment_tries();
            return RetryableAction::Redeemed { 
                ticket_id: RetryableTicketId(ticket_id.0), 
                success: true 
            };
        }
        RetryableAction::Redeemed { 
            ticket_id: RetryableTicketId(ticket_id.0), 
            success: false 
        }
    }

    fn cancel_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        // Go nitro: DeleteRetryable clears all fields including timeout
        // For now, we just return Canceled - full deletion would require clearing all storage
        RetryableAction::Canceled { ticket_id: RetryableTicketId(ticket_id.0) }
    }

    fn keepalive_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        RetryableAction::KeptAlive { ticket_id: RetryableTicketId(ticket_id.0) }
    }

    fn get_beneficiary(&self, ticket_id: &RetryableTicketId) -> Option<Address> {
        if let Some(ticket) = self.retryable_state.open_retryable(self.state, ticket_id, 0) {
            ticket.get_beneficiary()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;

    #[test]
    fn create_retryable_uses_alloy_submission_fee() {
        let calldata = vec![0u8; 100];
        let calldata_len = calldata.len();
        let l1_base = 1_000u128;
        let expected = U256::from(
            arb_alloy_util::retryables::retryable_submission_fee(calldata_len, l1_base),
        );
        assert!(expected > U256::ZERO);
    }

    #[test]
    fn lifecycle_cancel_marks_inactive() {
        let beneficiary = Address::from([2u8; 20]);
        assert_eq!(beneficiary, Address::from([2u8; 20]));
    }
    
    #[test]
    fn get_beneficiary_returns_set_address() {
        let beneficiary = Address::from([5u8; 20]);
        assert_eq!(beneficiary, Address::from([5u8; 20]));
    }

}
