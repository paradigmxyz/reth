use alloy_rpc_types_eth::BlockError;
use jsonrpsee_types::error::ErrorObject;
use reth_evm::execute::ProviderError;
use reth_rpc_eth_api::{AsEthApiError, EthTxEnvError, TransactionConversionError};
use reth_rpc_eth_types::EthApiError;
use reth_rpc_eth_types::error::api::FromEvmHalt;
use revm::context_interface::result::EVMError;


#[derive(Debug, thiserror::Error)]
pub enum ArbEthApiError {
    #[error(transparent)]
    Eth(#[from] EthApiError),
}

impl AsEthApiError for ArbEthApiError {
    fn as_err(&self) -> Option<&EthApiError> {
        match self {
            Self::Eth(err) => Some(err),
        }
    }
}

impl From<ArbEthApiError> for ErrorObject<'static> {
    fn from(err: ArbEthApiError) -> Self {
        match err {
            ArbEthApiError::Eth(e) => e.into(),
        }
    }
}


impl From<TransactionConversionError> for ArbEthApiError {
    fn from(value: TransactionConversionError) -> Self {
        Self::Eth(EthApiError::from(value))
    }
}

impl From<EthTxEnvError> for ArbEthApiError {
    fn from(value: EthTxEnvError) -> Self {
        Self::Eth(EthApiError::from(value))
    }
}

impl From<ProviderError> for ArbEthApiError {
    fn from(value: ProviderError) -> Self {
        Self::Eth(EthApiError::from(value))
    }
}

impl From<BlockError> for ArbEthApiError {
    fn from(value: BlockError) -> Self {
        Self::Eth(EthApiError::from(value))
    }
}

impl<H> FromEvmHalt<H> for ArbEthApiError
where
    EthApiError: FromEvmHalt<H>,
{
    fn from_evm_halt(halt: H, gas_limit: u64) -> Self {
        Self::Eth(EthApiError::from_evm_halt(halt, gas_limit))
    }
}

impl From<core::convert::Infallible> for ArbEthApiError {
    fn from(value: core::convert::Infallible) -> Self {
        match value {}
    }
}
impl From<EVMError<ProviderError>> for ArbEthApiError {
    fn from(value: EVMError<ProviderError>) -> Self {
        Self::Eth(EthApiError::from(value))
    }
}
