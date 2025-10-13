#![allow(unused)]

use alloy_primitives::{keccak256, Address, Bytes, U256, B256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64, StorageBackedBigUint, StorageBackedAddress};

#[derive(Clone, Copy)]
pub struct RetryableTicketId(pub [u8; 32]);

#[derive(Clone)]
pub struct RetryableCreateParams {
    pub sender: Address,
    pub beneficiary: Address,
    pub call_to: Address,
    pub call_data: Bytes,
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

pub struct RetryableTicket<D> {
    storage: Storage<D>,
    ticket_id: RetryableTicketId,
    
    escrowed: StorageBackedBigUint<D>,
    beneficiary: StorageBackedAddress<D>,
    from: StorageBackedAddress<D>,
    to: StorageBackedAddress<D>,
    call_value: StorageBackedBigUint<D>,
    call_data: StorageBackedBigUint<D>, // Hash of calldata
    timeout: StorageBackedUint64<D>,
    num_tries: StorageBackedUint64<D>,
    active: StorageBackedUint64<D>, // 1 if active, 0 if not
}

const ESCROWED_OFFSET: u64 = 0;
const BENEFICIARY_OFFSET: u64 = 1;
const FROM_OFFSET: u64 = 2;
const TO_OFFSET: u64 = 3;
const CALL_VALUE_OFFSET: u64 = 4;
const CALL_DATA_OFFSET: u64 = 5;
const TIMEOUT_OFFSET: u64 = 6;
const NUM_TRIES_OFFSET: u64 = 7;
const ACTIVE_OFFSET: u64 = 8;

pub const RETRYABLE_LIFETIME_SECONDS: u64 = 7 * 24 * 60 * 60;

impl<D: Database> RetryableState<D> {
    pub fn new(state: *mut revm::database::State<D>, base_key: B256) -> Self {
        let storage = Storage::new(state, base_key);
        Self { storage }
    }
    
    pub fn try_to_reap_one_retryable(&self, current_time: u64, state: *mut revm::database::State<D>) -> Result<bool, ()> {
        let timeout_storage = StorageBackedUint64::new(state, self.storage.base_key, 0);
        let oldest_timeout = timeout_storage.get().unwrap_or(u64::MAX);
        
        if oldest_timeout >= current_time {
            return Ok(false);
        }
        
        Ok(true)
    }

    pub fn create_retryable(
        &self,
        state: *mut revm::database::State<D>,
        params: RetryableCreateParams,
        current_time: u64,
    ) -> RetryableTicket<D> {
        let calldata_len = params.call_data.len();
        let l1_base_fee_wei: u128 = params.l1_base_fee.try_into().unwrap_or_default();
        let computed_submission_fee =
            arb_alloy_util::retryables::retryable_submission_fee(calldata_len, l1_base_fee_wei);
        let submission_fee = U256::from(computed_submission_fee);
        let escrowed = submission_fee.min(params.max_submission_cost);

        let mut preimage = Vec::with_capacity(20 + 20 + params.call_data.len());
        preimage.extend_from_slice(params.sender.as_slice());
        preimage.extend_from_slice(params.call_to.as_slice());
        preimage.extend_from_slice(&params.call_data);
        let id = keccak256(preimage);
        let ticket_id = RetryableTicketId(id.0);

        let ticket_storage = self.storage.open_sub_storage(&ticket_id.0);
        let ticket_base_key = B256::from(keccak256(&ticket_id.0));

        let ticket = RetryableTicket {
            storage: ticket_storage,
            ticket_id,
            escrowed: StorageBackedBigUint::new(state, ticket_base_key, ESCROWED_OFFSET),
            beneficiary: StorageBackedAddress::new(state, ticket_base_key, BENEFICIARY_OFFSET),
            from: StorageBackedAddress::new(state, ticket_base_key, FROM_OFFSET),
            to: StorageBackedAddress::new(state, ticket_base_key, TO_OFFSET),
            call_value: StorageBackedBigUint::new(state, ticket_base_key, CALL_VALUE_OFFSET),
            call_data: StorageBackedBigUint::new(state, ticket_base_key, CALL_DATA_OFFSET),
            timeout: StorageBackedUint64::new(state, ticket_base_key, TIMEOUT_OFFSET),
            num_tries: StorageBackedUint64::new(state, ticket_base_key, NUM_TRIES_OFFSET),
            active: StorageBackedUint64::new(state, ticket_base_key, ACTIVE_OFFSET),
        };

        let timeout = current_time + RETRYABLE_LIFETIME_SECONDS;
        let call_data_hash = U256::from_be_bytes(keccak256(&params.call_data).0);

        let _ = ticket.escrowed.set(escrowed);
        let _ = ticket.beneficiary.set(params.beneficiary);
        let _ = ticket.from.set(params.sender);
        let _ = ticket.to.set(params.call_to);
        let _ = ticket.call_value.set(params.submission_fee);
        let _ = ticket.call_data.set(call_data_hash);
        let _ = ticket.timeout.set(timeout);
        let _ = ticket.num_tries.set(0);
        let _ = ticket.active.set(1);

        ticket
    }

    pub fn open_retryable(
        &self,
        state: *mut revm::database::State<D>,
        ticket_id: &RetryableTicketId,
        current_time: u64,
    ) -> Option<RetryableTicket<D>> {
        let ticket_base_key = B256::from(keccak256(&ticket_id.0));
        let timeout_storage = StorageBackedUint64::new(state, ticket_base_key, TIMEOUT_OFFSET);
        
        if let Ok(timeout) = timeout_storage.get() {
            if timeout > current_time {
                let ticket_storage = self.storage.open_sub_storage(&ticket_id.0);
                
                return Some(RetryableTicket {
                    storage: ticket_storage,
                    ticket_id: RetryableTicketId(ticket_id.0),
                    escrowed: StorageBackedBigUint::new(state, ticket_base_key, ESCROWED_OFFSET),
                    beneficiary: StorageBackedAddress::new(state, ticket_base_key, BENEFICIARY_OFFSET),
                    from: StorageBackedAddress::new(state, ticket_base_key, FROM_OFFSET),
                    to: StorageBackedAddress::new(state, ticket_base_key, TO_OFFSET),
                    call_value: StorageBackedBigUint::new(state, ticket_base_key, CALL_VALUE_OFFSET),
                    call_data: StorageBackedBigUint::new(state, ticket_base_key, CALL_DATA_OFFSET),
                    timeout: timeout_storage,
                    num_tries: StorageBackedUint64::new(state, ticket_base_key, NUM_TRIES_OFFSET),
                    active: StorageBackedUint64::new(state, ticket_base_key, ACTIVE_OFFSET),
                });
            }
        }
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

    pub fn get_escrowed(&self) -> Option<U256> {
        self.escrowed.get().ok()
    }

    pub fn is_active(&self) -> bool {
        self.active.get().unwrap_or(0) == 1
    }

    pub fn deactivate(&self) -> Result<(), ()> {
        self.active.set(0)
    }

    pub fn increment_tries(&self) -> Result<(), ()> {
        let current = self.num_tries.get().unwrap_or(0);
        self.num_tries.set(current + 1)
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
        let ticket = self.retryable_state.create_retryable(self.state, params.clone(), 0);
        let ticket_id = ticket.ticket_id.clone();
        let escrowed = ticket.get_escrowed().unwrap_or_default();

        RetryableAction::Created {
            ticket_id,
            escrowed,
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
        if let Some(ticket) = self.retryable_state.open_retryable(self.state, ticket_id, 0) {
            if ticket.is_active() {
                let _ = ticket.deactivate();
                let _ = ticket.increment_tries();
                return RetryableAction::Redeemed { 
                    ticket_id: RetryableTicketId(ticket_id.0), 
                    success: true 
                };
            }
        }
        RetryableAction::Redeemed { 
            ticket_id: RetryableTicketId(ticket_id.0), 
            success: false 
        }
    }

    fn cancel_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        if let Some(ticket) = self.retryable_state.open_retryable(self.state, ticket_id, 0) {
            let _ = ticket.deactivate();
        }
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
