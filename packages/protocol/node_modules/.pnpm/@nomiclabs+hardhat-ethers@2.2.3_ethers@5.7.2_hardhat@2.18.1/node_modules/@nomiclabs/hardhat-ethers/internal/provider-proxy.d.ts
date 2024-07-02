import { EthereumProvider } from "hardhat/types";
import { EthersProviderWrapper } from "./ethers-provider-wrapper";
/**
 * This method returns a proxy that uses an underlying provider for everything.
 *
 * This underlying provider is replaced by a new one after a successful hardhat_reset,
 * because ethers providers can have internal state that returns wrong results after
 * the network is reset.
 */
export declare function createProviderProxy(hardhatProvider: EthereumProvider): EthersProviderWrapper;
//# sourceMappingURL=provider-proxy.d.ts.map