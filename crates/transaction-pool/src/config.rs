///! Configuration options for the Transaction pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    // TODO add limits for subpools
    // TODO limits for per peer
    // TODO config whether to check if transactions are banned
}

impl Default for PoolConfig {
    fn default() -> Self {
        todo!()
    }
}
