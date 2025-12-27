use core::cell::RefCell;

thread_local! {
    static SCHEDULED_TXS: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
}

pub fn clear() {
    SCHEDULED_TXS.with(|b| b.borrow_mut().clear());
}

pub fn push(encoded_tx: Vec<u8>) {
    SCHEDULED_TXS.with(|b| {
        b.borrow_mut().push(encoded_tx);
        tracing::info!(
            target: "arb-reth::scheduled-tx-sink",
            total_scheduled = b.borrow().len(),
            "Pushed scheduled transaction to sink"
        );
    });
}

pub fn take() -> Vec<Vec<u8>> {
    SCHEDULED_TXS.with(|b| {
        let mut v = b.borrow_mut();
        let out = v.clone();
        if !out.is_empty() {
            tracing::info!(
                target: "arb-reth::scheduled-tx-sink",
                scheduled_count = out.len(),
                "Taking scheduled transactions from sink"
            );
        }
        v.clear();
        out
    })
}
