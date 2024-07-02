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
exports.FakeSenderTransaction = void 0;
const rlp = __importStar(require("@nomicfoundation/ethereumjs-rlp"));
const ethereumjs_tx_1 = require("@nomicfoundation/ethereumjs-tx");
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const errors_1 = require("../../../core/providers/errors");
const makeFakeSignature_1 = require("../utils/makeFakeSignature");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
/**
 * This class represents a legacy transaction sent by a sender whose private
 * key we don't control.
 *
 * The transaction's signature is never validated, but assumed to be valid.
 *
 * The sender's private key is never recovered from the signature. Instead,
 * the sender's address is received as parameter.
 */
class FakeSenderTransaction extends ethereumjs_tx_1.Transaction {
    static fromTxData(_txData, _opts) {
        throw new errors_1.InternalError("`fromTxData` is not implemented in FakeSenderTransaction");
    }
    static fromSerializedTx(_serialized, _opts) {
        throw new errors_1.InternalError("`fromSerializedTx` is not implemented in FakeSenderTransaction");
    }
    static fromRlpSerializedTx(_serialized, _opts) {
        throw new errors_1.InternalError("`fromRlpSerializedTx` is not implemented in FakeSenderTransaction");
    }
    static fromValuesArray(_values, _opts) {
        throw new errors_1.InternalError("`fromRlpSerializedTx` is not implemented in FakeSenderTransaction");
    }
    static fromSenderAndRlpSerializedTx(sender, serialized, opts) {
        const values = (0, ethereumjs_util_1.arrToBufArr)(rlp.decode(serialized));
        checkIsFlatBufferArray(values);
        return this.fromSenderAndValuesArray(sender, values, opts);
    }
    static fromSenderAndValuesArray(sender, values, opts) {
        if (values.length !== 6 && values.length !== 9) {
            throw new errors_1.InternalError("FakeSenderTransaction initialized with invalid values");
        }
        const [nonce, gasPrice, gasLimit, to, value, data, v, r, s] = values;
        return new FakeSenderTransaction(sender, {
            nonce,
            gasPrice,
            gasLimit,
            to: to !== undefined && to.length > 0 ? to : undefined,
            value,
            data,
            v,
            r,
            s,
        }, opts);
    }
    constructor(sender, data = {}, opts) {
        const fakeSignature = (0, makeFakeSignature_1.makeFakeSignature)(data, sender);
        super({
            ...data,
            v: data.v ?? 27,
            r: data.r ?? fakeSignature.r,
            s: data.s ?? fakeSignature.s,
        }, { ...opts, freeze: false, disableMaxInitCodeSizeCheck: true });
        this.common = this._getCommon(opts?.common);
        this._sender = sender;
    }
    verifySignature() {
        return true;
    }
    getSenderAddress() {
        return this._sender;
    }
    sign() {
        throw new errors_1.InternalError("`sign` is not implemented in FakeSenderTransaction");
    }
    getSenderPublicKey() {
        throw new errors_1.InternalError("`getSenderPublicKey` is not implemented in FakeSenderTransaction");
    }
    getMessageToVerifySignature() {
        throw new errors_1.InternalError("`getMessageToVerifySignature` is not implemented in FakeSenderTransaction");
    }
    getMessageToSign() {
        throw new errors_1.InternalError("`getMessageToSign` is not implemented in FakeSenderTransaction");
    }
    validate(stringError = false) {
        if (stringError) {
            return [];
        }
        return true;
    }
}
exports.FakeSenderTransaction = FakeSenderTransaction;
// Override private methods
const FakeSenderTransactionPrototype = FakeSenderTransaction.prototype;
FakeSenderTransactionPrototype._validateTxV = function (_v, common) {
    return this._getCommon(common);
};
FakeSenderTransactionPrototype._signedTxImplementsEIP155 = function () {
    throw new errors_1.InternalError("`_signedTxImplementsEIP155` is not implemented in FakeSenderTransaction");
};
FakeSenderTransactionPrototype._unsignedTxImplementsEIP155 = function () {
    throw new errors_1.InternalError("`_unsignedTxImplementsEIP155` is not implemented in FakeSenderTransaction");
};
FakeSenderTransactionPrototype._getMessageToSign = function () {
    throw new errors_1.InternalError("`_getMessageToSign` is not implemented in FakeSenderTransaction");
};
FakeSenderTransactionPrototype._processSignature = function () {
    throw new errors_1.InternalError("`_processSignature` is not implemented in FakeSenderTransaction");
};
function checkIsFlatBufferArray(values) {
    if (!Array.isArray(values)) {
        throw new errors_1.InvalidArgumentsError(`Invalid deserialized tx. Expected a Buffer[], but got '${values}'`);
    }
    for (const [i, value] of values.entries()) {
        if (!Buffer.isBuffer(value)) {
            throw new errors_1.InvalidArgumentsError(`Invalid deserialized tx. Expected a Buffer in position ${i}, but got '${value}'`);
        }
    }
}
//# sourceMappingURL=FakeSenderTransaction.js.map