use core::cell::RefCell;
use alloy_primitives::{Address, B256, Log};

thread_local! {
    static PREDEPLOY_LOGS: RefCell<Vec<Log>> = RefCell::new(Vec::new());
}

pub fn clear() {
    PREDEPLOY_LOGS.with(|b| b.borrow_mut().clear());
}

pub fn push(address: Address, topics: &[[u8; 32]], data: &[u8]) {
    let t: Vec<B256> = topics.iter().map(|w| B256::from(*w)).collect();
    let log = Log::new_unchecked(address, t.clone(), alloy_primitives::Bytes::copy_from_slice(data));
    PREDEPLOY_LOGS.with(|b| {
        b.borrow_mut().push(log);
        tracing::info!(
            target: "arb-reth::log-sink",
            address = ?address,
            topics = ?t,
            data_len = data.len(),
            total_logs = b.borrow().len(),
            "Pushed log to sink"
        );
    });
}

pub fn take() -> Vec<Log> {
    PREDEPLOY_LOGS.with(|b| {
        let mut v = b.borrow_mut();
        let out = v.clone();
        tracing::info!(
            target: "arb-reth::log-sink",
            logs_count = out.len(),
            "Taking logs from sink"
        );
        v.clear();
        out
    })
}
