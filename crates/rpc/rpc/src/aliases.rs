use reth_evm::ConfigureEvm;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_types::EthApiError;

/// Boxed RPC converter.
pub type DynRpcConverter<Evm, Network, Error = EthApiError> = Box<
    dyn RpcConvert<
        Primitives = <Evm as ConfigureEvm>::Primitives,
        Network = Network,
        Error = Error,
        Evm = Evm,
    >,
>;
