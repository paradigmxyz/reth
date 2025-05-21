use revm::precompile::PrecompileError;

/// BSC specific precompile errors.
#[derive(Debug, PartialEq)]
pub enum BscPrecompileError {
    /// The cometbft validation input is invalid.
    CometBftInvalidInput,
    /// The cometbft apply block failed.
    CometBftApplyBlockFailed,
    /// The cometbft consensus state encoding failed.
    CometBftEncodeConsensusStateFailed,
    /// Reverted error
    /// This is for BSC EVM compatibility specially.
    /// This error will not consume all gas but only the returned amount.
    Reverted(u64),
}

impl From<BscPrecompileError> for PrecompileError {
    fn from(error: BscPrecompileError) -> Self {
        match error {
            BscPrecompileError::CometBftInvalidInput => {
                PrecompileError::Other("invalid input".to_string())
            }
            BscPrecompileError::CometBftApplyBlockFailed => {
                PrecompileError::Other("apply block failed".to_string())
            }
            BscPrecompileError::CometBftEncodeConsensusStateFailed => {
                PrecompileError::Other("encode consensus state failed".to_string())
            }
            BscPrecompileError::Reverted(gas) => PrecompileError::Other(format!("Reverted({gas})")),
        }
    }
}
