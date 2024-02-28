use jsonrpsee::proc_macros::rpc;
use reth_rpc_types::{
    SendBundleRequest, SendBundleResponse, SimBundleOverrides, SimBundleResponse,
};

/// Mev rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "mev"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "mev"))]
pub trait MevApi {
    /// Submitting bundles to the relay. It takes in a bundle and provides a bundle hash as a
    /// return value.
    #[method(name = "sendBundle")]
    async fn send_bundle(
        &self,
        request: SendBundleRequest,
    ) -> jsonrpsee::core::RpcResult<SendBundleResponse>;

    /// Similar to `mev_sendBundle` but instead of submitting a bundle to the relay, it returns
    /// a simulation result. Only fully matched bundles can be simulated.
    #[method(name = "simBundle")]
    async fn sim_bundle(
        &self,
        bundle: SendBundleRequest,
        sim_overrides: SimBundleOverrides,
    ) -> jsonrpsee::core::RpcResult<SimBundleResponse>;
}
