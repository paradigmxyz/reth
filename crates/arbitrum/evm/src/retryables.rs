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
        let _ = params;
        RetryableAction::Created { ticket_id: RetryableTicketId([0u8; 32]), escrowed: U256::ZERO }
    }

    fn redeem_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        let _ = ticket_id;
        RetryableAction::Redeemed { ticket_id: RetryableTicketId([0u8; 32]), success: false }
    }

    fn cancel_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        let _ = ticket_id;
        RetryableAction::Canceled { ticket_id: RetryableTicketId([0u8; 32]) }
    }

    fn keepalive_retryable(&mut self, ticket_id: &RetryableTicketId) -> RetryableAction {
        let _ = ticket_id;
        RetryableAction::KeptAlive { ticket_id: RetryableTicketId([0u8; 32]) }
    }
}
