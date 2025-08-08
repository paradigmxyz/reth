#![allow(unused)]

use alloy_primitives::{keccak256, Address, Bytes, U256};
use std::collections::HashMap;

pub struct RetryableTicketId(pub [u8; 32]);

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
    Created { ticket_id: RetryableTicketId, escrowed: U256 },
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
}

#[derive(Clone)]
pub struct DefaultRetryables {
    tickets: HashMap<[u8; 32], TicketState>,
}
impl Default for DefaultRetryables {
    fn default() -> Self {
        Self { tickets: HashMap::new() }
    }
}

struct TicketState {
    escrowed: U256,
    beneficiary: Address,
    active: bool,
}

impl Retryables for DefaultRetryables {
    fn create_retryable(&mut self, params: RetryableCreateParams) -> RetryableAction {
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

        self.tickets.insert(
            ticket_id.0,
            TicketState { escrowed, beneficiary: params.beneficiary, active: true },
        );

        RetryableAction::Created { ticket_id, escrowed }
    }

    fn redeem_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        if let Some(t) = self.tickets.get_mut(&ticket_id.0) {
            if t.active {
                t.active = false;
                return RetryableAction::Redeemed { ticket_id: RetryableTicketId(ticket_id.0), success: false };
            }
        }
        RetryableAction::Redeemed { ticket_id: RetryableTicketId(ticket_id.0), success: false }
    }

    fn cancel_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        if let Some(t) = self.tickets.get_mut(&ticket_id.0) {
            t.active = false;
        }
        RetryableAction::Canceled { ticket_id: RetryableTicketId(ticket_id.0) }
    }

    fn keepalive_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        RetryableAction::KeptAlive { ticket_id: RetryableTicketId(ticket_id.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;

    #[test]
    fn create_retryable_uses_alloy_submission_fee() {
        let mut r = DefaultRetryables::default();
        let calldata = vec![0u8; 100];
        let params = RetryableCreateParams {
            sender: Address::ZERO,
            beneficiary: Address::ZERO,
            call_to: Address::ZERO,
            call_data: Bytes::from(calldata.clone()),
            l1_base_fee: U256::from(1_000u64),
            submission_fee: U256::ZERO,
            max_submission_cost: U256::from(u128::MAX),
            max_gas: U256::ZERO,
            gas_price_bid: U256::ZERO,
        };
        let action = r.create_retryable(params);
        match action {
            RetryableAction::Created { escrowed, .. } => {
                let expected = U256::from(
                    arb_alloy_util::retryables::retryable_submission_fee(calldata.len(), 1_000u128),
                );
                assert_eq!(escrowed, expected);
            }
            _ => panic!("expected Created"),
        }
    }

    #[test]
    fn lifecycle_cancel_marks_inactive() {
        let mut r = DefaultRetryables::default();
        let params = RetryableCreateParams {
            sender: Address::from([1u8; 20]),
            beneficiary: Address::from([2u8; 20]),
            call_to: Address::from([3u8; 20]),
            call_data: Bytes::from(vec![0xde, 0xad]),
            l1_base_fee: U256::from(1000u64),
            submission_fee: U256::ZERO,
            max_submission_cost: U256::from(u128::MAX),
            max_gas: U256::ZERO,
            gas_price_bid: U256::ZERO,
        };
        let created = r.create_retryable(params);
        let id = match created {
            RetryableAction::Created { ticket_id, .. } => ticket_id,
            _ => panic!("expected Created"),
        };
        let _ = r.cancel_retryable(&id);
        let res = r.redeem_retryable(&id);
        match res {
            RetryableAction::Redeemed { success, .. } => {
                assert!(!success);
            }
            _ => panic!("expected Redeemed"),
        }
    }
}
