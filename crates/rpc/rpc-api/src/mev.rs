use alloy_rpc_types_mev::{
    SendBundleRequest, SendBundleResponse, SimBundleOverrides, SimBundleResponse,
};
use jsonrpsee::proc_macros::rpc;

/// Mev rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "mev"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "mev"))]
pub trait MevSimApi {
    /// Similar to `mev_sendBundle` but instead of submitting a bundle to the relay, it returns
    /// a simulation result. Only fully matched bundles can be simulated.
    #[method(name = "simBundle")]
    async fn sim_bundle(
        &self,
        bundle: SendBundleRequest,
        sim_overrides: SimBundleOverrides,
    ) -> jsonrpsee::core::RpcResult<SimBundleResponse>;
}

/// Mev rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "mev"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "mev"))]
pub trait MevFullApi {
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
