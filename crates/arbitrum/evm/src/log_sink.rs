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
    let log = Log::new_unchecked(address, t, alloy_primitives::Bytes::copy_from_slice(data));
    PREDEPLOY_LOGS.with(|b| b.borrow_mut().push(log));
}

pub fn take() -> Vec<Log> {
    PREDEPLOY_LOGS.with(|b| {
        let mut v = b.borrow_mut();
        let out = v.clone();
        v.clear();
        out
    })
}
