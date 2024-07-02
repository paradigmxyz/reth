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
exports.FakeSenderAccessListEIP2930Transaction = void 0;
const rlp = __importStar(require("@nomicfoundation/ethereumjs-rlp"));
const ethereumjs_tx_1 = require("@nomicfoundation/ethereumjs-tx");
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const errors_1 = require("../../../core/providers/errors");
const makeFakeSignature_1 = require("../utils/makeFakeSignature");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
/**
 * This class is the EIP-2930 version of FakeSenderTransaction.
 */
class FakeSenderAccessListEIP2930Transaction extends ethereumjs_tx_1.AccessListEIP2930Transaction {
    static fromTxData(_txData, _opts) {
        throw new errors_1.InternalError("`fromTxData` is not implemented in FakeSenderAccessListEIP2930Transaction");
    }
    static fromSerializedTx(_serialized, _opts) {
        throw new errors_1.InternalError("`fromSerializedTx` is not implemented in FakeSenderAccessListEIP2930Transaction");
    }
    static fromRlpSerializedTx(_serialized, _opts) {
        throw new errors_1.InternalError("`fromRlpSerializedTx` is not implemented in FakeSenderAccessListEIP2930Transaction");
    }
    static fromValuesArray(_values, _opts) {
        throw new errors_1.InternalError("`fromValuesArray` is not implemented in FakeSenderAccessListEIP2930Transaction");
    }
    static fromSenderAndRlpSerializedTx(sender, serialized, opts) {
        if (serialized[0] !== 1) {
            throw new errors_1.InvalidArgumentsError(`Invalid serialized tx input: not an EIP-2930 transaction (wrong tx type, expected: 1, received: ${serialized[0]}`);
        }
        const values = (0, ethereumjs_util_1.arrToBufArr)(rlp.decode(serialized.slice(1)));
        checkIsAccessListEIP2930ValuesArray(values);
        return this.fromSenderAndValuesArray(sender, values, opts);
    }
    static fromSenderAndValuesArray(sender, values, opts = {}) {
        const [chainId, nonce, gasPrice, gasLimit, to, value, data, accessList, v, r, s,] = values;
        return new FakeSenderAccessListEIP2930Transaction(sender, {
            chainId,
            nonce,
            gasPrice,
            gasLimit,
            to: to !== undefined && to.length > 0 ? to : undefined,
            value,
            data: data ?? Buffer.from([]),
            accessList: accessList ?? [],
            v: v !== undefined ? (0, ethereumjs_util_1.bufferToInt)(v) : undefined,
            r: r !== undefined && r.length !== 0 ? (0, ethereumjs_util_1.bufferToInt)(r) : undefined,
            s: s !== undefined && s.length !== 0 ? (0, ethereumjs_util_1.bufferToInt)(s) : undefined,
        }, opts);
    }
    constructor(sender, data = {}, opts) {
        const fakeSignature = (0, makeFakeSignature_1.makeFakeSignature)(data, sender);
        super({
            ...data,
            v: data.v ?? 1,
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
    getSenderPublicKey() {
        throw new errors_1.InternalError("`getSenderPublicKey` is not implemented in FakeSenderAccessListEIP2930Transaction");
    }
    _processSignature(_v, _r, _s) {
        throw new errors_1.InternalError("`_processSignature` is not implemented in FakeSenderAccessListEIP2930Transaction");
    }
    sign(_privateKey) {
        throw new errors_1.InternalError("`sign` is not implemented in FakeSenderAccessListEIP2930Transaction");
    }
    getMessageToSign() {
        throw new errors_1.InternalError("`getMessageToSign` is not implemented in FakeSenderAccessListEIP2930Transaction");
    }
    getMessageToVerifySignature() {
        throw new errors_1.InternalError("`getMessageToVerifySignature` is not implemented in FakeSenderAccessListEIP2930Transaction");
    }
    validate(stringError = false) {
        if (stringError) {
            return [];
        }
        return true;
    }
}
exports.FakeSenderAccessListEIP2930Transaction = FakeSenderAccessListEIP2930Transaction;
function checkIsAccessListEIP2930ValuesArray(values) {
    if (!Array.isArray(values)) {
        throw new errors_1.InvalidArgumentsError(`Invalid deserialized tx. Expected a Buffer[], but got '${values}'`);
    }
    if (values.length !== 8 && values.length !== 11) {
        throw new errors_1.InvalidArgumentsError("Invalid EIP-2930 transaction. Only expecting 8 values (for unsigned tx) or 11 values (for signed tx).");
    }
    // all elements in the array are buffers, except the 8th one that is an
    // AccessListBuffer (an array of AccessListBufferItems)
    for (const [i, value] of values.entries()) {
        if (i === 7) {
            if (!Array.isArray(value)) {
                // we could check more things to assert that it's an AccessListBuffer,
                // but we're assuming that just checking if it's an array is enough
                throw new errors_1.InvalidArgumentsError(`Invalid deserialized tx. Expected a AccessListBuffer in position ${i}, but got '${value}'`);
            }
        }
        else {
            if (!Buffer.isBuffer(values[i])) {
                throw new errors_1.InvalidArgumentsError(`Invalid deserialized tx. Expected a Buffer in position ${i}, but got '${value}'`);
            }
        }
    }
}
//# sourceMappingURL=FakeSenderAccessListEIP2930Transaction.js.map