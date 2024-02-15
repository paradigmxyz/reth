#[macro_export]
macro_rules! init_debug_propagated {
    () => {
        #[cfg(not(debug_assertions))]
        let (
            mut propagated_tx_hashes_count,
            mut propagated_txns_count,
            mut propagated_tx_hashes_peers_count,
            mut propagated_txns_peers_count,
        ) = (0, 0, 0, 0);

        #[cfg(debug_assertions)]
        let (
            mut propagated_tx_hashes,
            mut propagated_txns,
            mut propagated_tx_hashes_peers,
            mut propagated_txns_peers,
        ) = (vec![], vec![], vec![], vec![]);
    };
}
#[macro_export]
macro_rules! debug_track_propagated_announcement {
    () => {
        #[cfg(not(debug_assertions))]
        {
            propagated_tx_hashes_count += new_pooled_hashes.len();
            propagated_tx_hashes_peers_count += 1;
        }

        #[cfg(debug_assertions)]
        {
            (&mut propagated_tx_hashes).extend(new_pooled_hashes.iter().copied());
            (&mut propagated_tx_hashes_peers).push(*peer_id);
        }
    };
}
#[macro_export]
macro_rules! debug_track_propagated_txns {
    () => {
        #[cfg(not(debug_assertions))]
        {
            propagated_txns_count += new_full_transactions.len();
            propagated_txns_peers_count += 1;
        }

        #[cfg(debug_assertions)]
        {
            propagated_txns.extend(new_full_transactions.iter().copied());
            propagated_txns_peers.push(*peer_id);
        }
    };
}

#[macro_export]
macro_rules! debug_log_propagated {
() => {
#[cfg(not(debug_assertions))]
debug!(target: "net::tx",
    propagated_txns_count=propagated_txns_count,
    propagated_txns_peers_count=propagated_txns_peers_count,
    propagated_tx_hashes_count=propagated_tx_hashes_count,
    propagated_full_txns_peers_count=propagated_tx_hashes_peers_count,
    "propagated txns and announcements to peers"
);
#[cfg(debug_assertions)]
debug!(target: "net::tx",
    propagated_txns_len=?propagated_txns.len(),
    propagated_txns_peers_len=?propagated_txns_peers.len(),
    propagated_tx_hashes_len=?propagated_tx_hashes.len(),
    propagated_txns_peers_len=?propagated_tx_hashes_peers.len(),
    propagated_txns=?propagated_txns,
    propagated_txns_peers=?propagated_txns_peers,
    propagated_tx_hashes=?propagated_tx_hashes,
    propagated_tx_hashes_peers=?propagated_tx_hashes_peers,
    "propagated txns and announcements to peers"
);

};
}