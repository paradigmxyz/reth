// TODO: move
/// CliqueConfig is the consensus engine configs for proof-of-authority based sealing.
#[derive(Debug)]
pub struct CliqueConfig {
    /// Number of seconds between blocks to enforce
    pub period: u64,
    /// Epoch length to reset votes and checkpoint
    pub epoch: u64,
}
