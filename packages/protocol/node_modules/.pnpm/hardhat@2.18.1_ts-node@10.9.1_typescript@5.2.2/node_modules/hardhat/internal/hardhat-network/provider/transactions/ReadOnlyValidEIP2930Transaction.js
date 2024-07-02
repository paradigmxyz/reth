"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || function (mod) {
    if (mod && mod.__esModule) return mod;
    var result = {};
    if (mod != null) for (var k in mod) if (k !== "default" && Object.prototype.hasOwnProperty.call(mod, k)) __createBinding(result, mod, k);
    __setModuleDefault(result, mod);
    return result;
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.ReadOnlyValidEIP2930Transaction = void 0;
const ethereumjs_common_1 = require("@nomicfoundation/ethereumjs-common");
const ethereumjs_tx_1 = require("@nomicfoundation/ethereumjs-tx");
const errors_1 = require("../../../core/providers/errors");
const BigIntUtils = __importStar(require("../../../util/bigint"));
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
/**
 * This class is like `ReadOnlyValidTransaction` but for
 * EIP-2930 (access list) transactions.
 */
class ReadOnlyValidEIP2930Transaction extends ethereumjs_tx_1.AccessListEIP2930Transaction {
    static fromTxData(_txData, _opts) {
        throw new errors_1.InternalError("`fromTxData` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    static fromSerializedTx(_serialized, _opts) {
        throw new errors_1.InternalError("`fromSerializedTx` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    static fromRlpSerializedTx(_serialized, _opts) {
        throw new errors_1.InternalError("`fromRlpSerializedTx` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    static fromValuesArray(_values, _opts) {
        throw new errors_1.InternalError("`fromRlpSerializedTx` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    constructor(sender, data = {}) {
        const fakeCommon = ethereumjs_common_1.Common.custom({
            chainId: BigIntUtils.fromBigIntLike(data.chainId),
        }, {
            hardfork: "berlin",
        });
        super(data, {
            freeze: false,
            disableMaxInitCodeSizeCheck: true,
            common: fakeCommon,
        });
        this.common = fakeCommon;
        this._sender = sender;
    }
    verifySignature() {
        return true;
    }
    getSenderAddress() {
        return this._sender;
    }
    sign() {
        throw new errors_1.InternalError("`sign` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    getDataFee() {
        throw new errors_1.InternalError("`getDataFee` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    getBaseFee() {
        throw new errors_1.InternalError("`getBaseFee` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    getUpfrontCost() {
        throw new errors_1.InternalError("`getUpfrontCost` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    validate(_stringError = false) {
        throw new errors_1.InternalError("`validate` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    toCreationAddress() {
        throw new errors_1.InternalError("`toCreationAddress` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    getSenderPublicKey() {
        throw new errors_1.InternalError("`getSenderPublicKey` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    getMessageToVerifySignature() {
        throw new errors_1.InternalError("`getMessageToVerifySignature` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
    getMessageToSign() {
        throw new errors_1.InternalError("`getMessageToSign` is not implemented in ReadOnlyValidEIP2930Transaction");
    }
}
exports.ReadOnlyValidEIP2930Transaction = ReadOnlyValidEIP2930Transaction;
//# sourceMappingURL=ReadOnlyValidEIP2930Transaction.js.map