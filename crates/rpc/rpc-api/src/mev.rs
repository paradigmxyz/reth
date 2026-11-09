use alloy_rpc_macros::rpc;
use alloy_rpc_types_mev::{EthBundleHash, MevSendBundle, SimBundleOverrides, SimBundleResponse};

/// Mev rpc interface.
#[rpc(server, namespace = "mev")]
pub trait MevSimApi {
    /// Similar to `mev_sendBundle` but instead of submitting a bundle to the relay, it returns
    /// a simulation result. Only fully matched bundles can be simulated.
    #[method(name = "simBundle")]
    async fn sim_bundle(
        &self,
        bundle: MevSendBundle,
        sim_overrides: SimBundleOverrides,
    ) -> alloy_rpc_server::RpcResult<SimBundleResponse>;
}

/// Mev rpc interface.
#[rpc(server, namespace = "mev")]
pub trait MevFullApi {
    /// Submitting bundles to the relay. It takes in a bundle and provides a bundle hash as a
    /// return value.
    #[method(name = "sendBundle")]
    async fn send_bundle(
        &self,
        request: MevSendBundle,
    ) -> alloy_rpc_server::RpcResult<EthBundleHash>;

    /// Similar to `mev_sendBundle` but instead of submitting a bundle to the relay, it returns
    /// a simulation result. Only fully matched bundles can be simulated.
    #[method(name = "simBundle")]
    async fn sim_bundle(
        &self,
        bundle: MevSendBundle,
        sim_overrides: SimBundleOverrides,
    ) -> alloy_rpc_server::RpcResult<SimBundleResponse>;
}
