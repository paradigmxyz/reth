import { ethers } from "ethers";
export declare class SignerWithAddress extends ethers.Signer {
    readonly address: string;
    private readonly _signer;
    static create(signer: ethers.providers.JsonRpcSigner): Promise<SignerWithAddress>;
    private constructor();
    getAddress(): Promise<string>;
    signMessage(message: string | ethers.utils.Bytes): Promise<string>;
    signTransaction(transaction: ethers.utils.Deferrable<ethers.providers.TransactionRequest>): Promise<string>;
    sendTransaction(transaction: ethers.utils.Deferrable<ethers.providers.TransactionRequest>): Promise<ethers.providers.TransactionResponse>;
    connect(provider: ethers.providers.Provider): SignerWithAddress;
    _signTypedData(...params: Parameters<ethers.providers.JsonRpcSigner["_signTypedData"]>): Promise<string>;
    toJSON(): string;
}
//# sourceMappingURL=signers.d.ts.map