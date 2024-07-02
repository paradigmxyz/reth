/// <reference types="node" />
import { Common } from "@nomicfoundation/ethereumjs-common";
import { Transaction, TxData, TxOptions } from "@nomicfoundation/ethereumjs-tx";
import { Address } from "@nomicfoundation/ethereumjs-util";
/**
 * This class is like `ReadOnlyValidTransaction` but for
 * a transaction with an unknown tx type.
 */
export declare class ReadOnlyValidUnknownTypeTransaction extends Transaction {
    static fromTxData(_txData: TxData, _opts?: TxOptions): never;
    static fromSerializedTx(_serialized: Buffer, _opts?: TxOptions): never;
    static fromRlpSerializedTx(_serialized: Buffer, _opts?: TxOptions): never;
    static fromValuesArray(_values: Buffer[], _opts?: TxOptions): never;
    readonly common: Common;
    private readonly _sender;
    private readonly _actualType;
    constructor(sender: Address, type: number, data?: TxData);
    get type(): number;
    verifySignature(): boolean;
    getSenderAddress(): Address;
    sign(): never;
    getDataFee(): never;
    getBaseFee(): never;
    getUpfrontCost(): never;
    validate(_stringError?: false): never;
    validate(_stringError: true): never;
    toCreationAddress(): never;
    getSenderPublicKey(): never;
    getMessageToVerifySignature(): never;
    getMessageToSign(): never;
}
//# sourceMappingURL=ReadOnlyValidUnknownTypeTransaction.d.ts.map