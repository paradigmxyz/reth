use alloy_primitives::B256;

/// File list to be downloaded with their hashes.
pub(crate) static DOWNLOAD_FILE_LIST: [[(&str, B256); 3]; 2] = [
    [
        ("static_file_transactions_0_499999", B256::ZERO),
        ("static_file_transactions_0_499999.off", B256::ZERO),
        ("static_file_transactions_0_499999.conf", B256::ZERO),
        // ("static_file_blockmeta_0_499999", B256::ZERO),
        // ("static_file_blockmeta_0_499999.off", B256::ZERO),
        // ("static_file_blockmeta_0_499999.conf", B256::ZERO),
    ],
    [
        ("static_file_transactions_500000_999999", B256::ZERO),
        ("static_file_transactions_500000_999999.off", B256::ZERO),
        ("static_file_transactions_500000_999999.conf", B256::ZERO),
        // ("static_file_blockmeta_500000_999999", B256::ZERO),
        // ("static_file_blockmeta_500000_999999.off", B256::ZERO),
        // ("static_file_blockmeta_500000_999999.conf", B256::ZERO),
    ],
];
