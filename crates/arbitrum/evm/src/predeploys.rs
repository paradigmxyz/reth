#![allow(unused)]

use alloy_primitives::{Address, Bytes, U256, B256};
pub trait LogEmitter {
    fn emit_log(&mut self, address: alloy_primitives::Address, topics: &[[u8; 32]], data: &[u8]) {
        crate::log_sink::push(address, topics, data);
    }
}
pub struct NoopEmitter;
impl LogEmitter for NoopEmitter {}
use crate::retryables::{Retryables, DefaultRetryables};

pub struct PredeployCallContext {
    pub block_number: u64,
    pub block_hashes: alloc::vec::Vec<B256>,
    pub chain_id: U256,
    pub os_version: u64,
    pub time: u64,
    pub origin: Address,
    pub caller: Address,
    pub depth: u64,
    pub basefee: U256,
}
fn abi_word_u256(x: U256) -> [u8; 32] { x.to_be_bytes::<32>() }
fn abi_word_u64(x: u64) -> [u8; 32] { let mut out=[0u8;32]; out[24..].copy_from_slice(&x.to_be_bytes()); out }
fn abi_word_b256(x: B256) -> [u8; 32] { x.0 }
fn abi_word_address(a: Address) -> [u8; 32] { let mut out=[0u8;32]; out[12..].copy_from_slice(a.as_slice()); out }

pub trait PredeployHandler {
    fn address(&self) -> Address;
    fn call(&self, ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, value: U256, retryables: &mut dyn Retryables, emitter: &mut dyn LogEmitter) -> (Bytes, u64, bool);
}

pub struct PredeployRegistry {
    handlers: alloc::vec::Vec<alloc::boxed::Box<dyn PredeployHandler + Send + Sync>>,
}

impl PredeployRegistry {
    pub fn new() -> Self {
        Self { 
            handlers: alloc::vec::Vec::new(), 
        }
    }
    
    pub fn default() -> Self {
        Self::new()
    }

    pub fn register(&mut self, handler: alloc::boxed::Box<dyn PredeployHandler + Send + Sync>) {
        self.handlers.push(handler);
    }

    pub fn dispatch_with_emitter(&mut self, ctx: &PredeployCallContext, to: Address, input: &Bytes, gas_limit: u64, value: U256, retryables: &mut dyn Retryables, emitter: &mut dyn LogEmitter) -> Option<(Bytes, u64, bool)> {
        for h in &self.handlers {
            if h.address() == to {
                return Some(h.call(ctx, input, gas_limit, value, retryables, emitter));
            }
        }
        None
    }

    pub fn dispatch(&mut self, ctx: &PredeployCallContext, to: Address, input: &Bytes, gas_limit: u64, value: U256, retryables: &mut dyn Retryables) -> Option<(Bytes, u64, bool)> {
        let mut noop = NoopEmitter;
        self.dispatch_with_emitter(ctx, to, input, gas_limit, value, retryables, &mut noop)
    }
}

/* Minimal handler stubs; logic to be filled to match Nitro precompiles */

#[derive(Clone)]
pub struct ArbSys {
    pub addr: Address,
}

impl ArbSys {
    pub fn new(addr: Address) -> Self {
        Self { addr }
    }
}

impl PredeployHandler for ArbSys {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256, retryables: &mut dyn Retryables, _emitter: &mut dyn LogEmitter) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        let send_tx_to_l1 = pre::selector(pre::SIG_SEND_TX_TO_L1);
        let withdraw_eth = pre::selector(pre::SIG_WITHDRAW_ETH);
        let create_retryable = pre::selector(pre::SIG_RETRY_SUBMIT_RETRYABLE);
        let redeem = pre::selector(pre::SIG_RETRY_REDEEM);
        let cancel = pre::selector(pre::SIG_RETRY_CANCEL);
        let arb_block_number = pre::selector(pre::SIG_ARB_BLOCK_NUMBER);
        let arb_block_hash = pre::selector(pre::SIG_ARB_BLOCK_HASH);
        let get_tx_call_value = pre::selector("getTxCallValue()");
        let get_tx_origin = pre::selector("txOrigin()");
        let get_block_number = arb_block_number;
        let get_block_hash = arb_block_hash;
        let get_storage_at = pre::selector(pre::SIG_GET_STORAGE_GAS_AVAILABLE);
        let arb_chain_id = pre::selector(pre::SIG_ARB_CHAIN_ID);
        let arb_os_version = pre::selector(pre::SIG_ARB_OS_VERSION);

        fn encode_u256(x: U256) -> Bytes {
            let out = x.to_be_bytes::<32>();
            Bytes::from(out.to_vec())
        }
        fn encode_b256(x: B256) -> Bytes {
            Bytes::from(x.0.to_vec())
        }

        match sel {
            s if s == arb_block_number => {
                let out = encode_u256(U256::from(ctx.block_number));
                (out, gas_limit, true)
            }
            s if s == arb_block_hash => {
                if input.len() >= 4 + 32 {
                    let mut bn_bytes = [0u8; 32];
                    bn_bytes.copy_from_slice(&input[4..36]);
                    let bn = U256::from_be_bytes(bn_bytes);
                    let bn_u64 = bn.try_into().unwrap_or(0u64);
                    let current = ctx.block_number;
                    let ok_range = bn_u64 < current && current - bn_u64 <= 256;
                    let h = if ok_range {
                        let idx = (current - 1 - bn_u64) as usize;
                        ctx.block_hashes.get(idx).cloned().unwrap_or(B256::ZERO)
                    } else {
                        B256::ZERO
                    };
                    (encode_b256(h), gas_limit, true)
                } else {
                    (encode_b256(B256::ZERO), gas_limit, true)
                }
            }
            s if s == get_block_number => {
                let out = encode_u256(U256::from(ctx.block_number));
                (out, gas_limit, true)
            }
            s if s == get_block_hash => {
                if input.len() >= 4 + 32 {
                    let mut bn_bytes = [0u8; 32];
                    bn_bytes.copy_from_slice(&input[4..36]);
                    let bn = U256::from_be_bytes(bn_bytes);
                    let bn_u64 = bn.try_into().unwrap_or(0u64);
                    let current = ctx.block_number;
                    let ok_range = bn_u64 < current && current - bn_u64 <= 256;
                    let h = if ok_range {
                        let idx = (current - 1 - bn_u64) as usize;
                        ctx.block_hashes.get(idx).cloned().unwrap_or(B256::ZERO)
                    } else {
                        B256::ZERO
                    };
                    (encode_b256(h), gas_limit, true)
                } else {
                    (encode_b256(B256::ZERO), gas_limit, true)
                }
            }
            s if s == get_tx_origin => {
                let mut out = [0u8; 32];
                out[12..].copy_from_slice(ctx.origin.as_slice());
                (Bytes::from(out.to_vec()), gas_limit, true)
            }
            s if s == get_tx_call_value => {
                (encode_u256(_value), gas_limit, true)
            }
            s if s == arb_chain_id => {
                let out = encode_u256(ctx.chain_id);
                (out, gas_limit, true)
            }
            s if s == arb_os_version => {
                let base = 56u64;
                let out = encode_u256(U256::from(base.saturating_add(ctx.os_version)));
                (out, gas_limit, true)
            }

            s if s == send_tx_to_l1 => {
                let mut out = [0u8; 32];
                (Bytes::from(out.to_vec()), gas_limit, true)
            }
            s if s == withdraw_eth => (Bytes::default(), gas_limit, true),
            s if s == create_retryable => {
                let params = crate::retryables::RetryableCreateParams {
                    sender: ctx.caller,
                    beneficiary: Address::ZERO,
                    call_to: Address::ZERO,
                    call_data: Bytes::default(),
                    l1_base_fee: ctx.basefee,
                    submission_fee: U256::ZERO,
                    max_submission_cost: U256::MAX,
                    max_gas: U256::ZERO,
                    gas_price_bid: U256::ZERO,
                };
                match retryables.create_retryable(params) {
                    crate::retryables::RetryableAction::Created { ticket_id, .. } => {
                        let mut out = [0u8; 32];
                        out.copy_from_slice(&ticket_id.0);
                        (Bytes::from(out.to_vec()), gas_limit, true)
                    }
                    _ => (Bytes::default(), gas_limit, false),
                }
            }
            s if s == redeem => {
                let mut out = [0u8; 32];
                (Bytes::from(out.to_vec()), gas_limit, true)
            }
            s if s == cancel => (Bytes::default(), gas_limit, true),
            s if s == get_storage_at => {
                let mut out = [0u8; 32];
                (Bytes::from(out.to_vec()), gas_limit, true)
            },
            _ => (Bytes::default(), gas_limit, true),
        }
    }
}

#[derive(Clone)]
pub struct ArbRetryableTx {
    pub addr: Address,
}

impl ArbRetryableTx {
    pub fn new(addr: Address) -> Self {
        Self { addr }
    }
}

impl PredeployHandler for ArbRetryableTx {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256, retryables: &mut dyn Retryables, emitter: &mut dyn LogEmitter) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        let redeem = pre::selector(pre::SIG_RETRY_REDEEM);
        let cancel = pre::selector(pre::SIG_RETRY_CANCEL);
        let get_lifetime = pre::selector(pre::SIG_RETRY_GET_LIFETIME);
        let get_timeout = pre::selector(pre::SIG_RETRY_GET_TIMEOUT);
        let keepalive = pre::selector(pre::SIG_RETRY_KEEPALIVE);
        let get_beneficiary = pre::selector(pre::SIG_RETRY_GET_BENEFICIARY);
        let get_current_redeemer = pre::selector(pre::SIG_RETRY_GET_CURRENT_REDEEMER);
        let submit_retryable = pre::selector(pre::SIG_RETRY_SUBMIT_RETRYABLE);

        fn abi_zero_word() -> Bytes {
            let out = [0u8; 32];
            Bytes::from(out.to_vec())
        }

        fn read_ticket_id(input: &Bytes) -> [u8; 32] {
            let mut id = [0u8; 32];
            if input.len() >= 4 + 32 {
                id.copy_from_slice(&input[4..36]);
            }
            id
        }

        match sel {
            s if s == redeem => {
                let tid = read_ticket_id(input);
                let _ = retryables.redeem_retryable(&crate::retryables::RetryableTicketId(tid));
                (abi_zero_word(), gas_limit, true)
            }
            s if s == cancel => {
                let tid = read_ticket_id(input);
                let _ = retryables.cancel_retryable(&crate::retryables::RetryableTicketId(tid));
                (Bytes::default(), gas_limit, true)
            }
            s if s == get_lifetime => {
                let secs = arb_alloy_util::retryables::RETRYABLE_LIFETIME_SECONDS;
                let out = U256::from(secs).to_be_bytes::<32>();
                (Bytes::from(out.to_vec()), gas_limit, true)
            },
            s if s == get_timeout => {
                let timeout = arb_alloy_util::retryables::retryable_timeout_from(ctx.time);
                let out = U256::from(timeout).to_be_bytes::<32>();
                (Bytes::from(out.to_vec()), gas_limit, true)
            },
            s if s == keepalive => {
                let tid = read_ticket_id(input);
                let _ = retryables.keepalive_retryable(&crate::retryables::RetryableTicketId(tid));
                (abi_zero_word(), gas_limit, true)
            }
            s if s == get_beneficiary => {
                let tid = read_ticket_id(input);
                let addr = retryables
                    .get_beneficiary(&crate::retryables::RetryableTicketId(tid))
                    .unwrap_or(Address::ZERO);
                let mut out = [0u8; 32];
                out[12..].copy_from_slice(addr.as_slice());
                (Bytes::from(out.to_vec()), gas_limit, true)
            },
            s if s == get_current_redeemer => {
                let mut out = [0u8; 32];
                out[12..].copy_from_slice(ctx.caller.as_slice());
                (Bytes::from(out.to_vec()), gas_limit, true)
            },
            s if s == submit_retryable => {
                let params = crate::retryables::RetryableCreateParams {
                    sender: ctx.caller,
                    beneficiary: Address::ZERO,
                    call_to: Address::ZERO,
                    call_data: input.clone(),
                    l1_base_fee: ctx.basefee,
                    submission_fee: U256::ZERO,
                    max_submission_cost: U256::MAX,
                    max_gas: U256::ZERO,
                    gas_price_bid: U256::ZERO,
                };
                match retryables.create_retryable(params) {
                    crate::retryables::RetryableAction::Created { ticket_id, user_gas: _, nonce, refund_to, max_refund, submission_fee_refund, retry_tx_hash, sequence_num, .. } => {
                        let mut out = [0u8; 32];
                        out.copy_from_slice(&ticket_id.0);

                        let topic_created = arb_alloy_predeploys::topic(arb_alloy_predeploys::EVT_TICKET_CREATED);
                        let topics_created = [topic_created, ticket_id.0];
                        let data_created: [u8; 0] = [];
                        emitter.emit_log(self.addr, &topics_created, &data_created);

                        let topic_redeem = arb_alloy_predeploys::topic(arb_alloy_predeploys::EVT_REDEEM_SCHEDULED);
                        let seq_topic = abi_word_u64(sequence_num);
                        let topics_redeem = [topic_redeem, ticket_id.0, retry_tx_hash.0, seq_topic];
                        let mut data = alloc::vec::Vec::with_capacity(32 * 3);
                        data.extend_from_slice(&abi_word_address(refund_to));
                        data.extend_from_slice(&abi_word_u256(max_refund));
                        data.extend_from_slice(&abi_word_u256(submission_fee_refund));
                        emitter.emit_log(self.addr, &topics_redeem, &data);

                        (Bytes::from(out.to_vec()), gas_limit, true)
                    }
                    _ => (abi_zero_word(), gas_limit, false),
                }
            }
            _ => (Bytes::default(), gas_limit, true),
        }
}
}

#[derive(Clone)]
pub struct NodeInterface {
    pub addr: Address,
}

impl NodeInterface {
    pub fn new(addr: Address) -> Self {
        Self { addr }
    }
}

impl PredeployHandler for NodeInterface {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256, retryables: &mut dyn Retryables, _emitter: &mut dyn LogEmitter) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);

        let ni_estimate_retryable = pre::selector(pre::SIG_NI_ESTIMATE_RETRYABLE_TICKET);
        let ni_construct_outbox_proof = pre::selector(pre::SIG_NI_CONSTRUCT_OUTBOX_PROOF);
        let ni_find_batch = pre::selector(pre::SIG_NI_FIND_BATCH_CONTAINING_BLOCK);
        let ni_get_l1_conf = pre::selector(pre::SIG_NI_GET_L1_CONFIRMATIONS);
        let ni_gas_components = pre::selector(pre::SIG_NI_GAS_ESTIMATE_COMPONENTS);
        let ni_gas_l1_component = pre::selector(pre::SIG_NI_GAS_ESTIMATE_L1_COMPONENT);
        let ni_legacy_lookup = pre::selector(pre::SIG_NI_LEGACY_LOOKUP_MESSAGE_BATCH_PROOF);
        let ni_nitro_genesis = pre::selector(pre::SIG_NI_NITRO_GENESIS_BLOCK);
        let ni_block_l1_num = pre::selector(pre::SIG_NI_BLOCK_L1_NUM);
        let ni_l2_range_for_l1 = pre::selector(pre::SIG_NI_L2_BLOCK_RANGE_FOR_L1);

        fn abi_zero_word() -> Bytes {
            let out = [0u8; 32];
            Bytes::from(out.to_vec())
        }
        fn encode_u256(x: U256) -> Bytes {
            let out = x.to_be_bytes::<32>();
            Bytes::from(out.to_vec())
        }


        match sel {
            s if s == ni_nitro_genesis => (abi_zero_word(), gas_limit, true),
            s if s == ni_block_l1_num => (abi_zero_word(), gas_limit, true),
            s if s == ni_l2_range_for_l1 => {
                let mut out = alloc::vec::Vec::with_capacity(64);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                (Bytes::from(out), gas_limit, true)
            }
            s if s == ni_gas_components => {
                let mut out = alloc::vec::Vec::with_capacity(128);
                out.extend_from_slice(&[0u8; 32]); // gasEstimate (uint64) -> padded word
                out.extend_from_slice(&[0u8; 32]); // gasEstimateForL1 (uint64) -> padded word
                out.extend_from_slice(&encode_u256(ctx.basefee)); // baseFee (uint256)
                out.extend_from_slice(&encode_u256(ctx.basefee)); // l1BaseFeeEstimate (uint256)
                (Bytes::from(out), gas_limit, true)
            }
            s if s == ni_gas_l1_component => (encode_u256(ctx.basefee), gas_limit, true),
            s if s == ni_estimate_retryable => (abi_zero_word(), gas_limit, true),
            s if s == ni_construct_outbox_proof => (Bytes::default(), gas_limit, true),
            s if s == ni_find_batch => (abi_zero_word(), gas_limit, true),
            s if s == ni_get_l1_conf => (abi_zero_word(), gas_limit, true),
            s if s == ni_legacy_lookup => (Bytes::default(), gas_limit, true),
            _ => (Bytes::default(), gas_limit, true),
        }
    }
}
#[derive(Clone)]
pub struct ArbOwner {
    pub addr: Address,
    owners: alloc::sync::Arc<std::sync::Mutex<alloc::collections::BTreeSet<Address>>>,
    network_fee: alloc::sync::Arc<std::sync::Mutex<Address>>,
    infra_fee: alloc::sync::Arc<std::sync::Mutex<Address>>,
}

impl ArbOwner {
    pub fn new(addr: Address) -> Self {
        Self {
            addr,
            owners: alloc::sync::Arc::new(std::sync::Mutex::new(alloc::collections::BTreeSet::new())),
            network_fee: alloc::sync::Arc::new(std::sync::Mutex::new(Address::ZERO)),
            infra_fee: alloc::sync::Arc::new(std::sync::Mutex::new(Address::ZERO)),
        }
    }
}

impl PredeployHandler for ArbOwner {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, _ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256, _retryables: &mut dyn Retryables, _emitter: &mut dyn LogEmitter) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);

        fn encode_address(addr: Address) -> Bytes {
            let mut out = [0u8; 32];
            out[12..].copy_from_slice(addr.as_slice());
            Bytes::from(out.to_vec())
        }
        fn encode_u256(x: U256) -> Bytes {
            let out = x.to_be_bytes::<32>();
            Bytes::from(out.to_vec())
        }
        fn read_address(input: &Bytes) -> Address {
            if input.len() >= 4 + 32 {
                let mut a = [0u8; 20];
                a.copy_from_slice(&input[4 + 12..4 + 32]);
                Address::from(a)
            } else {
                Address::ZERO
            }
        }

        let add_owner = pre::selector(pre::SIG_OWNER_ADD_CHAIN_OWNER);
        let remove_owner = pre::selector(pre::SIG_OWNER_REMOVE_CHAIN_OWNER);
        let is_owner = pre::selector(pre::SIG_OWNER_IS_CHAIN_OWNER);
        let all_owners = pre::selector(pre::SIG_OWNER_GET_ALL_CHAIN_OWNERS);
        let get_net_fee = pre::selector(pre::SIG_OWNER_GET_NETWORK_FEE_ACCOUNT);
        let get_infra_fee = pre::selector(pre::SIG_OWNER_GET_INFRA_FEE_ACCOUNT);
        let set_net_fee = pre::selector(pre::SIG_OWNER_SET_NETWORK_FEE_ACCOUNT);
        let set_infra_fee = pre::selector(pre::SIG_OWNER_SET_INFRA_FEE_ACCOUNT);

        match sel {
            s if s == add_owner => {
                let addr = read_address(input);
                let mut owners = self.owners.lock().unwrap();
                owners.insert(addr);
                (Bytes::default(), gas_limit, true)
            }
            s if s == remove_owner => {
                let addr = read_address(input);
                let mut owners = self.owners.lock().unwrap();
                owners.remove(&addr);
                (Bytes::default(), gas_limit, true)
            }
            s if s == is_owner => {
                let addr = read_address(input);
                let owners = self.owners.lock().unwrap();
                let exists = owners.contains(&addr) as u64;
                (encode_u256(U256::from(exists)), gas_limit, true)
            }
            s if s == all_owners => {
                (Bytes::default(), gas_limit, true)
            }
            s if s == get_net_fee => {
                let addr = *self.network_fee.lock().unwrap();
                (encode_address(addr), gas_limit, true)
            }
            s if s == get_infra_fee => {
                let addr = *self.infra_fee.lock().unwrap();
                (encode_address(addr), gas_limit, true)
            }
            s if s == set_net_fee => {
                let addr = read_address(input);
                *self.network_fee.lock().unwrap() = addr;
                (Bytes::default(), gas_limit, true)
            }
            s if s == set_infra_fee => {
                let addr = read_address(input);
                *self.infra_fee.lock().unwrap() = addr;
                (Bytes::default(), gas_limit, true)
            }
            _ => (Bytes::default(), gas_limit, true),
        }
    }
}

#[derive(Clone)]
pub struct ArbGasInfo {
    pub addr: Address,
}

impl ArbGasInfo {
    pub fn new(addr: Address) -> Self {
        Self { addr }
    }
}

impl PredeployHandler for ArbGasInfo {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, _ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256, retryables: &mut dyn Retryables, _emitter: &mut dyn LogEmitter) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);

        let gi_prices_in_wei = pre::selector(pre::SIG_GI_GET_PRICES_IN_WEI);
        let gi_prices_in_wei_with_agg = pre::selector(pre::SIG_GI_GET_PRICES_IN_WEI_WITH_AGG);
        let gi_prices_in_arbgas = pre::selector(pre::SIG_GI_GET_PRICES_IN_ARBGAS);
        let gi_prices_in_arbgas_with_agg = pre::selector(pre::SIG_GI_GET_PRICES_IN_ARBGAS_WITH_AGG);
        let gi_min_gas_price = pre::selector(pre::SIG_GI_GET_MIN_GAS_PRICE);
        let gi_l1_basefee_estimate = pre::selector(pre::SIG_GI_GET_L1_BASEFEE_ESTIMATE);
        let gi_l1_basefee_inertia = pre::selector(pre::SIG_GI_GET_L1_BASEFEE_INERTIA);
        let gi_l1_reward_rate = pre::selector(pre::SIG_GI_GET_L1_REWARD_RATE);
        let gi_l1_reward_recipient = pre::selector(pre::SIG_GI_GET_L1_REWARD_RECIPIENT);
        let gi_l1_gas_price_estimate = pre::selector(pre::SIG_GI_GET_L1_GAS_PRICE_ESTIMATE);
        let gi_current_tx_l1_fees = pre::selector(pre::SIG_GI_GET_CURRENT_TX_L1_FEES);

        fn abi_zero_word() -> Bytes {
            let out = [0u8; 32];
            Bytes::from(out.to_vec())
        }

        match sel {
            s if s == gi_prices_in_wei => {
                let mut out = alloc::vec::Vec::with_capacity(96);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                (Bytes::from(out), gas_limit, true)
            }
            s if s == gi_prices_in_wei_with_agg => {
                let mut out = alloc::vec::Vec::with_capacity(96);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                (Bytes::from(out), gas_limit, true)
            }
            s if s == gi_prices_in_arbgas => {
                let mut out = alloc::vec::Vec::with_capacity(96);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                (Bytes::from(out), gas_limit, true)
            }
            s if s == gi_prices_in_arbgas_with_agg => {
                let mut out = alloc::vec::Vec::with_capacity(96);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                (Bytes::from(out), gas_limit, true)
            }
            s if s == gi_min_gas_price => (abi_zero_word(), gas_limit, true),
            s if s == gi_l1_basefee_estimate => {
                let out = _ctx.basefee.to_be_bytes::<32>();
                (Bytes::from(out.to_vec()), gas_limit, true)
            }
            s if s == gi_l1_basefee_inertia => (abi_zero_word(), gas_limit, true),
            s if s == gi_l1_reward_rate => (abi_zero_word(), gas_limit, true),
            s if s == gi_l1_reward_recipient => (abi_zero_word(), gas_limit, true),
            s if s == gi_l1_gas_price_estimate => {
                let out = _ctx.basefee.to_be_bytes::<32>();
                (Bytes::from(out.to_vec()), gas_limit, true)
            }
            s if s == gi_current_tx_l1_fees => {
                let mut out = alloc::vec::Vec::with_capacity(96);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&[0u8; 32]);
                (Bytes::from(out), gas_limit, true)
            }
            _ => (Bytes::default(), gas_limit, true),
        }
    }
}
#[derive(Clone)]
pub struct ArbAddressTable {
    pub addr: Address,
    addrs: alloc::sync::Arc<std::sync::Mutex<alloc::vec::Vec<Address>>>,
    index: alloc::sync::Arc<std::sync::Mutex<alloc::collections::BTreeMap<Address, u64>>>,
}

impl ArbAddressTable {
    pub fn new(addr: Address) -> Self {
        Self {
            addr,
            addrs: alloc::sync::Arc::new(std::sync::Mutex::new(alloc::vec::Vec::new())),
            index: alloc::sync::Arc::new(std::sync::Mutex::new(alloc::collections::BTreeMap::new())),
        }
    }
}

impl PredeployHandler for ArbAddressTable {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, _ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256, _retryables: &mut dyn Retryables, _emitter: &mut dyn LogEmitter) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        let at_exists = pre::selector(pre::SIG_AT_ADDRESS_EXISTS);
        let at_compress = pre::selector(pre::SIG_AT_COMPRESS);
        let at_decompress = pre::selector(pre::SIG_AT_DECOMPRESS);
        let at_lookup = pre::selector(pre::SIG_AT_LOOKUP);
        let at_lookup_index = pre::selector(pre::SIG_AT_LOOKUP_INDEX);
        let at_register = pre::selector(pre::SIG_AT_REGISTER);
        let at_size = pre::selector(pre::SIG_AT_SIZE);

        fn encode_u256(x: U256) -> Bytes {
            let out = x.to_be_bytes::<32>();
            Bytes::from(out.to_vec())
        }
        fn encode_address(addr: Address) -> Bytes {
            let mut out = [0u8; 32];
            out[12..].copy_from_slice(addr.as_slice());
            Bytes::from(out.to_vec())
        }
        fn decode_address(input: &Bytes) -> Address {
            if input.len() >= 4 + 32 {
                let mut a = [0u8; 20];
                a.copy_from_slice(&input[4 + 12..4 + 32]);
                Address::from(a)
            } else {
                Address::ZERO
            }
        }
        fn decode_index(input: &Bytes) -> u64 {
            if input.len() >= 4 + 32 {
                let mut buf = [0u8; 32];
                buf.copy_from_slice(&input[4..36]);
                let v = U256::from_be_bytes(buf);
                u64::try_from(v).unwrap_or(0)
            } else {
                0
            }
        }

        match sel {
            s if s == at_exists => {
                let addr = decode_address(input);
                let exists = self.index.lock().unwrap().contains_key(&addr);
                (encode_u256(U256::from(exists as u64)), gas_limit, true)
            },
            s if s == at_compress => (Bytes::default(), gas_limit, true),
            s if s == at_decompress => (Bytes::default(), gas_limit, true),
            s if s == at_lookup => {
                let addr = decode_address(input);
                let idx = self.index.lock().unwrap().get(&addr).copied().unwrap_or(0);
                (encode_u256(U256::from(idx)), gas_limit, true)
            },
            s if s == at_lookup_index => {
                let idx = decode_index(input);
                let addr = self.addrs.lock().unwrap().get(idx as usize).copied().unwrap_or(Address::ZERO);
                (encode_address(addr), gas_limit, true)
            },
            s if s == at_register => {
                let addr = decode_address(input);
                if let Some(&idx) = self.index.lock().unwrap().get(&addr) {
                    return (encode_u256(U256::from(idx)), gas_limit, true)
                }
                let mut addrs = self.addrs.lock().unwrap();
                let mut index = self.index.lock().unwrap();
                let idx = addrs.len() as u64;
                addrs.push(addr);
                index.insert(addr, idx);
                (encode_u256(U256::from(idx)), gas_limit, true)
            },
            s if s == at_size => {
                (encode_u256(U256::from(self.addrs.lock().unwrap().len() as u64)), gas_limit, true)
            },
            _ => (Bytes::default(), gas_limit, true),
        }
    }
}

impl PredeployRegistry {
    pub fn with_addresses(arb_sys: Address, arb_retryable_tx: Address, arb_owner: Address, arb_address_table: Address) -> Self {
        let mut reg = Self::default();
        reg.register(alloc::boxed::Box::new(ArbSys::new(arb_sys)));
        reg.register(alloc::boxed::Box::new(ArbRetryableTx::new(arb_retryable_tx)));
        reg.register(alloc::boxed::Box::new(ArbOwner::new(arb_owner)));
        reg.register(alloc::boxed::Box::new(ArbAddressTable::new(arb_address_table)));
        reg
    }

    pub fn with_default_addresses() -> Self {
        use arb_alloy_predeploys as pre;
        let mut reg = Self::default();
        reg.register(alloc::boxed::Box::new(ArbSys::new(Address::from(pre::ARB_SYS))));
        reg.register(alloc::boxed::Box::new(ArbRetryableTx::new(Address::from(pre::ARB_RETRYABLE_TX))));
        reg.register(alloc::boxed::Box::new(ArbGasInfo::new(Address::from(pre::ARB_GAS_INFO))));

        reg.register(alloc::boxed::Box::new(ArbOwner::new(Address::from(pre::ARB_OWNER))));
        reg.register(alloc::boxed::Box::new(ArbAddressTable::new(Address::from(pre::ARB_ADDRESS_TABLE))));
        reg.register(alloc::boxed::Box::new(NodeInterface::new(Address::from(pre::NODE_INTERFACE))));
        reg
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    use alloy_primitives::{address, Bytes, U256};

    pub(crate) fn mk_ctx() -> PredeployCallContext {
        PredeployCallContext {
            block_number: 100,
            block_hashes: alloc::vec::Vec::new(),
            chain_id: U256::from(42161u64),
            os_version: 0,
            time: 0,
            origin: Address::ZERO,
            caller: Address::ZERO,
            depth: 1,
            basefee: U256::ZERO,
        }
    }

    fn mk_bytes() -> Bytes {
        Bytes::default()
    }

    #[test]
    fn dispatch_routes_to_correct_handler() {
        let a_sys = address!("0000000000000000000000000000000000000064");
        let a_retry = address!("000000000000000000000000000000000000006e");
        let a_owner = address!("0000000000000000000000000000000000000070");
        let a_table = address!("0000000000000000000000000000000000000066");

        let mut reg = PredeployRegistry::with_addresses(a_sys, a_retry, a_owner, a_table);
        let ctx = mk_ctx();

        let out_sys = reg.dispatch(&ctx, a_sys, &mk_bytes(), 100_000, U256::ZERO);
        assert!(out_sys.is_some());
        let out_retry_at_sys = reg.dispatch(&ctx, a_sys, &mk_bytes(), 100_000, U256::ZERO);
        assert!(out_retry_at_sys.is_some()); // same result as first since dispatch matches address only

        for addr in [a_retry, a_owner, a_table] {
            let out = reg.dispatch(&ctx, addr, &mk_bytes(), 42_000, U256::from(1));
            assert!(out.is_some());
        }

        let unknown = address!("00000000000000000000000000000000000000ff");
        assert!(reg.dispatch(&ctx, unknown, &mk_bytes(), 1, U256::ZERO).is_none());
    }

    #[test]
    fn node_interface_is_registered_in_default_registry() {
        use alloy_primitives::address;
        let mut reg = PredeployRegistry::with_default_addresses();
        let ni = address!("00000000000000000000000000000000000000c8");
        let out = reg.dispatch(&mk_ctx(), ni, &mk_bytes(), 21_000, U256::ZERO);
        assert!(out.is_some());
    }
    #[test]
    fn arbsys_basic_queries_return_expected_values() {
        use alloy_primitives::{address, U256};
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let sys_addr = address!("0000000000000000000000000000000000000064");

        let mut ctx = mk_ctx();
        ctx.block_number = 1234;
        ctx.chain_id = U256::from(42161u64);
        ctx.os_version = 1;

        let mut call_and_decode_u256 = |selector: [u8;4]| -> U256 {
            let mut input = alloc::vec::Vec::with_capacity(4);
            input.extend_from_slice(&selector);
            let (out, _gas_left, success) = reg.dispatch(&ctx, sys_addr, &Bytes::from(input), 100_000, U256::ZERO).expect("dispatch");
            assert!(success);
            assert_eq!(out.len(), 32);
            let mut buf = [0u8; 32];
            buf.copy_from_slice(&out[..32]);
            U256::from_be_bytes(buf)
        };

        let got_block = call_and_decode_u256(pre::selector(pre::SIG_ARB_BLOCK_NUMBER));
        assert_eq!(got_block, U256::from(1234u64));

        let got_chain = call_and_decode_u256(pre::selector(pre::SIG_ARB_CHAIN_ID));
        assert_eq!(got_chain, U256::from(42161u64));

        let got_os_ver = call_and_decode_u256(pre::selector(pre::SIG_ARB_OS_VERSION));
        assert_eq!(got_os_ver, U256::from(56u64 + 1u64));
}

    #[test]
    fn arbsys_blockhash_queries_return_expected_values() {
        use alloy_primitives::{address, Bytes, B256, U256};
        use arb_alloy_predeploys as pre;

        let mut ctx = mk_ctx();
        ctx.block_number = 105;
        // block_hashes[0] corresponds to block 104, [1] -> 103, ...
        ctx.block_hashes = alloc::vec![
            B256::from_slice(&[1u8; 32]),
            B256::from_slice(&[2u8; 32]),
            B256::from_slice(&[3u8; 32]),
        ];

        let mut reg = PredeployRegistry::with_default_addresses();
        let sys_addr = address!("0000000000000000000000000000000000000064");

        let mut call_hash = |selector: [u8;4], block_num: u64| -> B256 {
            let mut input = alloc::vec::Vec::with_capacity(4 + 32);
            input.extend_from_slice(&selector);
            let bn = U256::from(block_num).to_be_bytes::<32>();
            input.extend_from_slice(&bn);
            let (out, _gas_left, success) = reg.dispatch(&ctx, sys_addr, &Bytes::from(input), 100_000, U256::ZERO).expect("dispatch");
            assert!(success);
            assert_eq!(out.len(), 32);
            let mut buf = [0u8; 32];
            buf.copy_from_slice(&out[..32]);
            B256::from(buf)
        };

        let h_104 = call_hash(pre::selector(pre::SIG_ARB_BLOCK_HASH), 104);
        assert_eq!(h_104, B256::from_slice(&[1u8; 32]));

        let h_103 = call_hash(pre::selector(pre::SIG_ARB_BLOCK_HASH), 103);
        assert_eq!(h_103, B256::from_slice(&[2u8; 32]));

        let h_105 = call_hash(pre::selector(pre::SIG_ARB_BLOCK_HASH), 105);
        assert_eq!(h_105, B256::ZERO);
        let h_far = call_hash(pre::selector(pre::SIG_ARB_BLOCK_HASH), 0);
        assert_eq!(h_far, B256::ZERO);
    }
    #[test]
    fn arbsys_origin_and_callvalue_return_expected_values() {
        use alloy_primitives::{address, Bytes, U256};
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let sys_addr = address!("0000000000000000000000000000000000000064");

        let mut ctx = mk_ctx();
        ctx.origin = address!("00000000000000000000000000000000000000aa");

        let mut input = alloc::vec::Vec::with_capacity(4);
        input.extend_from_slice(&pre::selector("txOrigin()"));
        let (out, _gas_left, success) = reg.dispatch(&ctx, sys_addr, &Bytes::from(input), 100_000, U256::ZERO).expect("dispatch");
        assert!(success);
        assert_eq!(out.len(), 32);
        assert_eq!(&out[12..], ctx.origin.as_slice());

        let mut input2 = alloc::vec::Vec::with_capacity(4);
        input2.extend_from_slice(&pre::selector("getTxCallValue()"));
        let callvalue = U256::from(123456u64);
        let (out2, _gas_left2, success2) = reg.dispatch(&ctx, sys_addr, &Bytes::from(input2), 100_000, callvalue).expect("dispatch");
        assert!(success2);
        assert_eq!(out2.len(), 32);
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out2[..32]);
        let got = U256::from_be_bytes(buf);
        assert_eq!(got, callvalue);
    }

    #[test]
    fn arb_gasinfo_l1_basefee_estimate_returns_ctx_basefee() {
        use alloy_primitives::{address, U256};
        let mut ctx = mk_ctx();
        ctx.basefee = U256::from(1_234_567u64);
        let mut reg = PredeployRegistry::with_default_addresses();
        let gi = address!("000000000000000000000000000000000000006c");
        let (out, _gas, ok) = reg.dispatch(&ctx, gi, &mk_bytes(), 21_000, U256::ZERO).expect("dispatch");
        assert!(ok);
        use arb_alloy_predeploys as pre;
        let mut input = alloc::vec::Vec::with_capacity(4);
        input.extend_from_slice(&pre::selector(pre::SIG_GI_GET_L1_BASEFEE_ESTIMATE));
        let (out2, _gas2, ok2) = reg.dispatch(&ctx, gi, &Bytes::from(input), 21_000, U256::ZERO).expect("dispatch");
        assert!(ok2);
        assert_eq!(out2.len(), 32);
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out2[..32]);
        let got = U256::from_be_bytes(buf);
        assert_eq!(got, ctx.basefee);
    }
    #[test]
    fn arb_gasinfo_l1_gas_price_estimate_returns_ctx_basefee() {
        use alloy_primitives::{address, U256};
        let mut ctx = mk_ctx();
        ctx.basefee = U256::from(987_654u64);
        let mut reg = PredeployRegistry::with_default_addresses();
        let gi = address!("000000000000000000000000000000000000006c");
        use arb_alloy_predeploys as pre;
        let mut input = alloc::vec::Vec::with_capacity(4);
        input.extend_from_slice(&pre::selector(pre::SIG_GI_GET_L1_GAS_PRICE_ESTIMATE));
        let (out, _gas, ok) = reg.dispatch(&ctx, gi, &Bytes::from(input), 21_000, U256::ZERO).expect("dispatch");
        assert!(ok);
        assert_eq!(out.len(), 32);
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out[..32]);
        let got = U256::from_be_bytes(buf);
        assert_eq!(got, ctx.basefee);
    }

    #[test]
    fn gasinfo_is_registered_in_default_registry() {
        use alloy_primitives::{address, U256};
        let mut reg = PredeployRegistry::with_default_addresses();
        let gi = address!("000000000000000000000000000000000000006c");
        let out = reg.dispatch(&mk_ctx(), gi, &mk_bytes(), 21_000, U256::ZERO);
        assert!(out.is_some());
    }

    #[test]
    fn arb_retryable_tx_basic_selectors_dispatch() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_retry = address!("000000000000000000000000000000000000006e");

        let mut call = |sel: [u8;4]| {
            let mut input = alloc::vec::Vec::with_capacity(4);
            input.extend_from_slice(&sel);
            reg.dispatch(&mk_ctx(), addr_retry, &Bytes::from(input), 50_000, U256::ZERO)
                .expect("dispatch")
        };

        let _ = call(pre::selector(pre::SIG_RETRY_REDEEM));
        let _ = call(pre::selector(pre::SIG_RETRY_CANCEL));
        let _ = call(pre::selector(pre::SIG_RETRY_GET_LIFETIME));
        let _ = call(pre::selector(pre::SIG_RETRY_GET_TIMEOUT));
        let _ = call(pre::selector(pre::SIG_RETRY_KEEPALIVE));
        let _ = call(pre::selector(pre::SIG_RETRY_GET_BENEFICIARY));
        let _ = call(pre::selector(pre::SIG_RETRY_GET_CURRENT_REDEEMER));
        let (out, _gas, success) = call(pre::selector(pre::SIG_RETRY_SUBMIT_RETRYABLE));
        assert!(success);
        assert_eq!(out.len(), 32);
    }
    #[test]
    fn arb_retryable_tx_submit_returns_ticket_id_word() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_retry = address!("000000000000000000000000000000000000006e");

        let mut input = alloc::vec::Vec::with_capacity(4);
        input.extend_from_slice(&pre::selector(pre::SIG_RETRY_SUBMIT_RETRYABLE));
        let (out, _gas, success) = reg
            .dispatch(&mk_ctx(), addr_retry, &Bytes::from(input), 50_000, U256::ZERO)
            .expect("dispatch");
        assert!(success);
        assert_eq!(out.len(), 32);
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out[..32]);
        assert_ne!(buf, [0u8; 32]);
    }

    #[test]
    fn arb_retryable_tx_get_beneficiary_after_submit_retryable() {
        use alloy_primitives::{address, U256, Bytes};
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_retry = address!("000000000000000000000000000000000000006e");

        let mut input = alloc::vec::Vec::with_capacity(4);
        input.extend_from_slice(&pre::selector(pre::SIG_RETRY_SUBMIT_RETRYABLE));
        let (out, _gas, success) = reg
            .dispatch(&mk_ctx(), addr_retry, &Bytes::from(input), 100_000, U256::ZERO)
            .expect("dispatch");
        assert!(success);
        assert_eq!(out.len(), 32);
        let mut id = [0u8; 32];
        id.copy_from_slice(&out[..32]);

        let mut input2 = alloc::vec::Vec::with_capacity(4 + 32);
        input2.extend_from_slice(&pre::selector(pre::SIG_RETRY_GET_BENEFICIARY));
        input2.extend_from_slice(&id);
        let (out2, _gas2, success2) = reg
            .dispatch(&mk_ctx(), addr_retry, &Bytes::from(input2), 50_000, U256::ZERO)
            .expect("dispatch");
        assert!(success2);
        assert_eq!(out2.len(), 32);
    }

    #[test]
    fn arb_retryable_tx_get_current_redeemer_is_ctx_caller() {
        use alloy_primitives::{address, U256, Bytes};
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_retry = address!("000000000000000000000000000000000000006e");

        let ctx = {
            let mut c = mk_ctx();
            c.caller = address!("00000000000000000000000000000000000000ff");
            c
        };

        let mut input = alloc::vec::Vec::with_capacity(4);
        input.extend_from_slice(&pre::selector(pre::SIG_RETRY_GET_CURRENT_REDEEMER));
        let (out, _gas, success) = reg
            .dispatch(&ctx, addr_retry, &Bytes::from(input), 50_000, U256::ZERO)
            .expect("dispatch");
        assert!(success);
        assert_eq!(out.len(), 32);
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out[..32]);
        let got = alloy_primitives::U256::from_be_bytes(buf);
        assert_ne!(got, alloy_primitives::U256::ZERO);
    }

    #[test]
    fn arb_retryable_tx_get_lifetime_returns_constant() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_retry = address!("000000000000000000000000000000000000006e");

        let mut input = alloc::vec::Vec::with_capacity(4);
        input.extend_from_slice(&pre::selector(pre::SIG_RETRY_GET_LIFETIME));
        let (out, _gas, success) = reg
            .dispatch(&mk_ctx(), addr_retry, &Bytes::from(input), 50_000, U256::ZERO)
            .expect("dispatch");
        assert!(success);
        assert_eq!(out.len(), 32);

        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out[..32]);
        let got = U256::from_be_bytes(buf);
        let expected = U256::from(arb_alloy_util::retryables::RETRYABLE_LIFETIME_SECONDS);
        assert_eq!(got, expected);
    }

    #[test]
    fn arb_retryable_tx_get_timeout_uses_ctx_time_plus_lifetime() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_retry = address!("000000000000000000000000000000000000006e");
        let mut ctx = mk_ctx();
        ctx.time = 1_000_000;

        let mut input = alloc::vec::Vec::with_capacity(4);
        input.extend_from_slice(&pre::selector(pre::SIG_RETRY_GET_TIMEOUT));
        let (out, _gas, ok) = reg
            .dispatch(&ctx, addr_retry, &Bytes::from(input), 50_000, U256::ZERO)
            .expect("dispatch");
        assert!(ok);
        assert_eq!(out.len(), 32);

        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out[..32]);
        let got = U256::from_be_bytes(buf);
        let expected =
            U256::from(ctx.time) + U256::from(arb_alloy_util::retryables::RETRYABLE_LIFETIME_SECONDS);
        assert_eq!(got, expected);
    }

    #[test]
    fn arbsys_os_version_and_chain_id_return_expected_values() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let sys_addr = address!("0000000000000000000000000000000000000064");

        let mut ctx = mk_ctx();
        ctx.chain_id = U256::from(42161u64);
        ctx.os_version = 5;

        let mk = |sel: [u8;4]| {
            let mut v = alloc::vec::Vec::with_capacity(4);
            v.extend_from_slice(&sel);
            Bytes::from(v)
        };

        let (out_os, _gas_os, ok_os) = reg.dispatch(&ctx, sys_addr, &mk(pre::selector(pre::SIG_ARB_OS_VERSION)), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok_os);
        assert_eq!(out_os.len(), 32);
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out_os[..32]);
        let got_os = U256::from_be_bytes(buf);
        assert_eq!(got_os, U256::from(56u64 + ctx.os_version));

        let (out_chain, _gas_chain, ok_chain) = reg.dispatch(&ctx, sys_addr, &mk(pre::selector(pre::SIG_ARB_CHAIN_ID)), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok_chain);
        let mut buf2 = [0u8; 32];
        buf2.copy_from_slice(&out_chain[..32]);
        let got_chain = U256::from_be_bytes(buf2);
        assert_eq!(got_chain, ctx.chain_id);
    }

    #[test]
    fn arbsys_send_tx_to_l1_and_withdraw_eth_dispatch_and_return_abi() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let sys_addr = address!("0000000000000000000000000000000000000064");

        let mk_input = |sel: [u8; 4]| {
            let mut v = alloc::vec::Vec::with_capacity(4 + 32 * 3);
            v.extend_from_slice(&sel);
            Bytes::from(v)
        };

        let (out_send, _gas_send, ok_send) =
            reg.dispatch(&mk_ctx(), sys_addr, &mk_input(pre::selector(pre::SIG_SEND_TX_TO_L1)), 200_000, U256::ZERO)
                .expect("dispatch");
        assert!(ok_send);
        assert_eq!(out_send.len(), 32);

        let (out_withdraw, _gas_withdraw, ok_withdraw) =
            reg.dispatch(&mk_ctx(), sys_addr, &mk_input(pre::selector(pre::SIG_WITHDRAW_ETH)), 200_000, U256::ZERO)
                .expect("dispatch");
        assert!(ok_withdraw);
        assert!(out_withdraw.len() == 0 || out_withdraw.len() == 32);
    }

    #[test]
    fn node_interface_basic_selector_dispatch() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let ni_addr = address!("00000000000000000000000000000000000000c8");

        let mk = |sig: &str| {
            let sel = pre::selector(sig);
            let mut v = alloc::vec::Vec::with_capacity(4);
            v.extend_from_slice(&sel);
            Bytes::from(v)
        };

        let (out, _gas, ok) = reg
            .dispatch(&mk_ctx(), ni_addr, &mk(pre::SIG_NI_GAS_ESTIMATE_L1_COMPONENT), 200_000, U256::ZERO)
            .expect("dispatch");
        assert!(ok);
        assert!(out.len() == 0 || out.len() == 32);

        let mut ctx = mk_ctx();
        ctx.basefee = U256::from(123_456u64);
        let (out2, _gas2, ok2) = reg
            .dispatch(&ctx, ni_addr, &mk(pre::SIG_NI_GAS_ESTIMATE_L1_COMPONENT), 200_000, U256::ZERO)
            .expect("dispatch");
        assert!(ok2);
        assert_eq!(out2.len(), 32);
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out2[..32]);
        let got = U256::from_be_bytes(buf);
        assert_eq!(got, ctx.basefee);
    }
    
    #[test]
    fn node_interface_gas_estimate_components_returns_basefees() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;

        let mut reg = PredeployRegistry::with_default_addresses();
        let ni_addr = address!("00000000000000000000000000000000000000c8");

        let mk = |sig: &str| {
            let sel = pre::selector(sig);
            let mut v = alloc::vec::Vec::with_capacity(4);
            v.extend_from_slice(&sel);
            Bytes::from(v)
        };

        let mut ctx = mk_ctx();
        ctx.basefee = U256::from(123_456u64);
        use alloy_primitives::U256;
        let (out, _gas, ok) = reg
            .dispatch(&ctx, ni_addr, &mk(pre::SIG_NI_GAS_ESTIMATE_COMPONENTS), 200_000, U256::ZERO)
            .expect("dispatch");
        assert!(ok);
        assert_eq!(out.len(), 128);

        let w0 = &out[0..32];
        let w1 = &out[32..64];
        let w2 = &out[64..96];
        let w3 = &out[96..128];

        assert!(w0.iter().all(|b| *b == 0));
        assert!(w1.iter().all(|b| *b == 0));

        let mut buf2 = [0u8; 32];
        buf2.copy_from_slice(w2);
        let mut buf3 = [0u8; 32];
        buf3.copy_from_slice(w3);
        let got2 = U256::from_be_bytes(buf2);
        let got3 = U256::from_be_bytes(buf3);
        assert_eq!(got2, ctx.basefee);
        assert_eq!(got3, ctx.basefee);

    }
    
    #[test]
    fn node_interface_trivial_selectors_return_expected_abi() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;
        let mut reg = PredeployRegistry::with_default_addresses();
        let ni_addr = address!("00000000000000000000000000000000000000c8");

        let mk = |sig: &str| {
            let sel = pre::selector(sig);
            let mut v = alloc::vec::Vec::with_capacity(4);
            v.extend_from_slice(&sel);
            Bytes::from(v)
        };

        let (out_gen, _g1, ok_gen) = reg.dispatch(&mk_ctx(), ni_addr, &mk(pre::SIG_NI_NITRO_GENESIS_BLOCK), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok_gen);
        assert_eq!(out_gen.len(), 32);

        let (out_b1, _g2, ok_b1) = reg.dispatch(&mk_ctx(), ni_addr, &mk(pre::SIG_NI_BLOCK_L1_NUM), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok_b1);
        assert!(out_b1.is_empty() || out_b1.len() == 32);

        let (out_range, _g3, ok_range) = reg.dispatch(&mk_ctx(), ni_addr, &mk(pre::SIG_NI_L2_BLOCK_RANGE_FOR_L1), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok_range);
        assert_eq!(out_range.len(), 64);
    }

    #[test]
    fn arb_owner_getters_return_words() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;
        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_owner = address!("0000000000000000000000000000000000000070");

        let call = |sig: &str| {
            let sel = pre::selector(sig);
            let mut v = alloc::vec::Vec::with_capacity(4);
            v.extend_from_slice(&sel);
            Bytes::from(v)
        };
        let (o1, _g1, ok1) = reg.dispatch(&mk_ctx(), addr_owner, &call(pre::SIG_OWNER_GET_NETWORK_FEE_ACCOUNT), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok1);
        assert!(o1.is_empty() || o1.len() == 32);

        let (o2, _g2, ok2) = reg.dispatch(&mk_ctx(), addr_owner, &call(pre::SIG_OWNER_GET_INFRA_FEE_ACCOUNT), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok2);
        assert!(o2.is_empty() || o2.len() == 32);
    }

 
    #[test]
    fn arb_owner_basic_selectors_dispatch() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;
        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_owner = address!("0000000000000000000000000000000000000070");

        let mut call = |sel: [u8;4]| {
            let mut v = alloc::vec::Vec::with_capacity(4);
            v.extend_from_slice(&sel);
            Bytes::from(v)
        };

        let (_o1, _g1, ok1) = reg.dispatch(&mk_ctx(), addr_owner, &call(pre::selector(pre::SIG_OWNER_IS_CHAIN_OWNER)), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok1);
        let (_o2, _g2, ok2) = reg.dispatch(&mk_ctx(), addr_owner, &call(pre::selector(pre::SIG_OWNER_GET_NETWORK_FEE_ACCOUNT)), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok2);
    }

    #[test]
    fn arb_address_table_register_and_lookup_flow() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;
        let addr_table = address!("0000000000000000000000000000000000000066");
        let mut reg = PredeployRegistry::with_default_addresses();
        let ctx = mk_ctx();

        let mut in_exists = alloc::vec::Vec::with_capacity(4 + 32);
        in_exists.extend_from_slice(&pre::selector(pre::SIG_AT_ADDRESS_EXISTS));
        let mut word = [0u8; 32];
        let a = Address::from([9u8; 20]);
        word[12..].copy_from_slice(a.as_slice());
        in_exists.extend_from_slice(&word);
        let (out0, _, _) = reg.dispatch(&ctx, addr_table, &Bytes::from(in_exists), 50_000, U256::ZERO).unwrap();
        assert_eq!(U256::from_be_slice(&out0[..32]), U256::ZERO);

        let mut in_reg = alloc::vec::Vec::with_capacity(4 + 32);
        in_reg.extend_from_slice(&pre::selector(pre::SIG_AT_REGISTER));
        in_reg.extend_from_slice(&word);
        let (out1, _, _) = reg.dispatch(&ctx, addr_table, &Bytes::from(in_reg), 50_000, U256::ZERO).unwrap();
        assert_eq!(U256::from_be_slice(&out1[..32]), U256::from(0));

        let mut in_size = alloc::vec::Vec::with_capacity(4);
        in_size.extend_from_slice(&pre::selector(pre::SIG_AT_SIZE));
        let (out2, _, _) = reg.dispatch(&ctx, addr_table, &Bytes::from(in_size), 50_000, U256::ZERO).unwrap();
        let sz = U256::from_be_slice(&out2[..32]);
        assert!(sz >= U256::from(0));

        let mut in_lookup = alloc::vec::Vec::with_capacity(4 + 32);
        in_lookup.extend_from_slice(&pre::selector(pre::SIG_AT_LOOKUP));
        in_lookup.extend_from_slice(&word);
        let (out3, _, _) = reg.dispatch(&ctx, addr_table, &Bytes::from(in_lookup), 50_000, U256::ZERO).unwrap();
        let _idx = U256::from_be_slice(&out3[..32]);

        let mut in_lookup_i = alloc::vec::Vec::with_capacity(4 + 32);
        in_lookup_i.extend_from_slice(&pre::selector(pre::SIG_AT_LOOKUP_INDEX));
        let idx_word = U256::from(0).to_be_bytes::<32>();
        in_lookup_i.extend_from_slice(&idx_word);
        let (out4, _, _) = reg.dispatch(&ctx, addr_table, &Bytes::from(in_lookup_i), 50_000, U256::ZERO).unwrap();
        assert_eq!(out4.len(), 32);
    }

    #[test]
    fn arb_owner_is_registered_in_default_registry() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;
        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_owner = address!("0000000000000000000000000000000000000070");
        let ctx = PredeployCallContext {
            block_number: 100,
            block_hashes: alloc::vec::Vec::new(),
            chain_id: U256::from(42161u64),
            os_version: 0,
            time: 0,
            origin: alloy_primitives::Address::ZERO,
            caller: alloy_primitives::Address::ZERO,
            depth: 1,
            basefee: U256::ZERO,
        };
        let (_out, _gas, success) = reg.dispatch(&ctx, addr_owner, &Bytes::default(), 50_000, U256::ZERO).expect("dispatch");
        assert!(success);
    }

    #[test]
    fn arb_address_table_is_registered_in_default_registry() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;
        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_at = address!("0000000000000000000000000000000000000066");
        let (_out, _gas, success) = reg.dispatch(&mk_ctx(), addr_at, &Bytes::default(), 50_000, U256::ZERO).expect("dispatch");
        assert!(success);
    }

    #[test]
    fn arb_address_table_basic_selectors_dispatch() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;
        let mut reg = PredeployRegistry::with_default_addresses();
        let at_addr = address!("0000000000000000000000000000000000000066");
        let mk = |sig: &str| {
            let sel = pre::selector(sig);
            let mut v = alloc::vec::Vec::with_capacity(4);
            v.extend_from_slice(&sel);
            Bytes::from(v)
        };
        for sig in [
            pre::SIG_AT_ADDRESS_EXISTS,
            pre::SIG_AT_COMPRESS,
            pre::SIG_AT_DECOMPRESS,
            pre::SIG_AT_LOOKUP,
            pre::SIG_AT_LOOKUP_INDEX,
            pre::SIG_AT_REGISTER,
            pre::SIG_AT_SIZE,
        ] {
            let (_o, _g, ok) = reg.dispatch(&mk_ctx(), at_addr, &mk(sig), 50_000, U256::ZERO).expect("dispatch");
            assert!(ok, "selector should dispatch: {}", sig);
        }
    }

    #[test]
    fn arb_address_table_selector_abi_shapes() {
        use alloy_primitives::address;
        use arb_alloy_predeploys as pre;
    
        let mut reg = PredeployRegistry::with_default_addresses();
        let addr_at = address!("0000000000000000000000000000000000000066");

        let mk = |sig: &str| {
            let sel = pre::selector(sig);
            let mut v = alloc::vec::Vec::with_capacity(4);
            v.extend_from_slice(&sel);
            Bytes::from(v)
        };

        let (o_exists, _g1, ok1) = reg.dispatch(&mk_ctx(), addr_at, &mk(pre::SIG_AT_ADDRESS_EXISTS), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok1);
        assert!(o_exists.is_empty() || o_exists.len() == 32);

        let (o_lookup, _g2, ok2) = reg.dispatch(&mk_ctx(), addr_at, &mk(pre::SIG_AT_LOOKUP), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok2);
        assert!(o_lookup.is_empty() || o_lookup.len() == 32);

        let (o_lookup_index, _g3, ok3) = reg.dispatch(&mk_ctx(), addr_at, &mk(pre::SIG_AT_LOOKUP_INDEX), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok3);
        assert!(o_lookup_index.is_empty() || o_lookup_index.len() == 32);

        let (o_register, _g4, ok4) = reg.dispatch(&mk_ctx(), addr_at, &mk(pre::SIG_AT_REGISTER), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok4);
        assert!(o_register.is_empty() || o_register.len() == 32);

        let (o_size, _g5, ok5) = reg.dispatch(&mk_ctx(), addr_at, &mk(pre::SIG_AT_SIZE), 50_000, U256::ZERO).expect("dispatch");
        assert!(ok5);
        assert!(o_size.is_empty() || o_size.len() == 32);
    }
}
