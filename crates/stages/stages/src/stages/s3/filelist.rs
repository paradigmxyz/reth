use alloy_primitives::B256;

/// File list to be downloaded with their hashes.
pub(crate) static DOWNLOAD_FILE_LIST: [[(&str, B256); 6]; 1] = [[
    ("static_file_transactions_0_499999", B256::ZERO),
    ("static_file_transactions_0_499999.off", B256::ZERO),
    ("static_file_transactions_0_499999.conf", B256::ZERO),
    ("static_file_bmeta_0_499999", B256::ZERO),
    ("static_file_bmeta_0_499999.off", B256::ZERO),
    ("static_file_bmeta_0_499999.conf", B256::ZERO),
]];
