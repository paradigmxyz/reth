//! Optimism-specific execution logic.

/// If the Deposited transaction failed, the deposit must still be included. In this case, we need
/// to increment the sender nonce and disregard the state changes. The transaction is also recorded
/// as using all gas unless it is a system transaction.
#[macro_export]
macro_rules! fail_deposit_tx {
    (
        $db:tt,
        $sender:ident,
        $block_number:ident,
        $transaction:ident,
        $post_state:ident,
        $cumulative_gas_used:ident,
        $is_regolith:ident,
        $error:expr
    ) => {
        let sender_account = $db.load_account($sender).map_err(|_| $error)?;
        let old_sender_info = to_reth_acc(&sender_account.info);
        sender_account.info.nonce += 1;
        let new_sender_info = to_reth_acc(&sender_account.info);

        $post_state.change_account($block_number, $sender, old_sender_info, new_sender_info);
        let sender_info = sender_account.info.clone();
        $db.insert_account_info($sender, sender_info);

        if $is_regolith || !$transaction.is_system_transaction() {
            $cumulative_gas_used += $transaction.gas_limit();
        }

        $post_state.add_receipt(
            $block_number,
            Receipt {
                tx_type: $transaction.tx_type(),
                success: false,
                cumulative_gas_used: $cumulative_gas_used,
                logs: vec![],
                // Deposit nonces are only recorded after Regolith
                deposit_nonce: $is_regolith.then_some(old_sender_info.nonce),
            },
        );
    };
}

pub use fail_deposit_tx;
