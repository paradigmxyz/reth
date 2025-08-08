#![allow(unused)]

use alloy_primitives::{Address, Bytes, U256};

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

#[derive(Default, Clone)]
pub struct DefaultRetryables;

impl Retryables for DefaultRetryables {
    fn create_retryable(&mut self, params: RetryableCreateParams) -> RetryableAction {
        let calldata_len = params.call_data.len();
        let l1_base_fee_wei: u128 = params.l1_base_fee.try_into().unwrap_or_default();
        let computed_submission_fee =
            arb_alloy_util::retryables::retryable_submission_fee(calldata_len, l1_base_fee_wei);
        let submission_fee = U256::from(computed_submission_fee);
        let escrowed = submission_fee.min(params.max_submission_cost);
        let ticket_id = RetryableTicketId([0u8; 32]);
        RetryableAction::Created { ticket_id, escrowed }
    }

    fn redeem_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        RetryableAction::Redeemed { ticket_id: RetryableTicketId(ticket_id.0), success: false }
    }

    fn cancel_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
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
}
