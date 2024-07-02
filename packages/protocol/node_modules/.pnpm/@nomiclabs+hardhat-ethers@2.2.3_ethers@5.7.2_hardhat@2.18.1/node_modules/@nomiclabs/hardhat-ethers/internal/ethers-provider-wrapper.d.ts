import { ethers } from "ethers";
import { EthereumProvider } from "hardhat/types";
export declare class EthersProviderWrapper extends ethers.providers.JsonRpcProvider {
    private readonly _hardhatProvider;
    constructor(hardhatProvider: EthereumProvider);
    send(method: string, params: any): Promise<any>;
    toJSON(): string;
}
//# sourceMappingURL=ethers-provider-wrapper.d.ts.map