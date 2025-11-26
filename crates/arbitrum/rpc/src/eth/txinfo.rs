use reth_storage_api::HeaderProvider;
use reth_rpc_convert::TxInfoMapper;

#[derive(Clone, Debug)]
pub struct ArbTxInfoMapper<P> {
    _provider: P,
}

impl<P> ArbTxInfoMapper<P> {
    pub fn new(provider: P) -> Self {
        Self { _provider: provider }
    }
}

impl<P, T> TxInfoMapper<T> for ArbTxInfoMapper<P>
where
    P: HeaderProvider + Clone + Send + Sync + 'static,
{
    type Out = alloy_rpc_types_eth::TransactionInfo;
    type Err = core::convert::Infallible;

    fn try_map(&self, _tx: &T, tx_info: alloy_rpc_types_eth::TransactionInfo) -> Result<Self::Out, Self::Err> {
        Ok(tx_info)
    }
}
