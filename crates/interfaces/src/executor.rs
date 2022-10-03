use reth_primitives::Block;

pub trait BlockExecutor {
    pub fn execute(&self, block: Block) -> Error {

    }
}

pub enum Error {
    VerificationFailed,
}