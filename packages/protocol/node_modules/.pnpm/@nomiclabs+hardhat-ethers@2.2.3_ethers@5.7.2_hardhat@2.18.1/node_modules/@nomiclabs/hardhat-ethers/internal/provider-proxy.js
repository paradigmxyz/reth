"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.createProviderProxy = void 0;
const constants_1 = require("hardhat/internal/constants");
const ethers_provider_wrapper_1 = require("./ethers-provider-wrapper");
const updatable_target_proxy_1 = require("./updatable-target-proxy");
/**
 * This method returns a proxy that uses an underlying provider for everything.
 *
 * This underlying provider is replaced by a new one after a successful hardhat_reset,
 * because ethers providers can have internal state that returns wrong results after
 * the network is reset.
 */
function createProviderProxy(hardhatProvider) {
    const initialProvider = new ethers_provider_wrapper_1.EthersProviderWrapper(hardhatProvider);
    const { proxy: providerProxy, setTarget } = (0, updatable_target_proxy_1.createUpdatableTargetProxy)(initialProvider);
    hardhatProvider.on(constants_1.HARDHAT_NETWORK_RESET_EVENT, () => {
        setTarget(new ethers_provider_wrapper_1.EthersProviderWrapper(hardhatProvider));
    });
    hardhatProvider.on(constants_1.HARDHAT_NETWORK_REVERT_SNAPSHOT_EVENT, () => {
        setTarget(new ethers_provider_wrapper_1.EthersProviderWrapper(hardhatProvider));
    });
    return providerProxy;
}
exports.createProviderProxy = createProviderProxy;
//# sourceMappingURL=provider-proxy.js.map