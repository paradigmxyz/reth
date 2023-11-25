use reth_libmdbx::metric::*;
use std::time::Duration;

/// start db record
pub fn start_reth_db_record() {
    start_db_record();
}

/// get reth_db record (read_size, read_time, write_size, write_time)
pub fn get_reth_db_record() -> (usize, Duration, usize, Duration) {
    get_db_record()
}
