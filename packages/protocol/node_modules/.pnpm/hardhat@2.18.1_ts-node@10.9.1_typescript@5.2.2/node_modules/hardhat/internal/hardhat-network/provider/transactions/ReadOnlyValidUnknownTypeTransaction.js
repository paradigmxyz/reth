"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ReadOnlyValidUnknownTypeTransaction = void 0;
const ethereumjs_tx_1 = require("@nomicfoundation/ethereumjs-tx");
const errors_1 = require("../../../core/providers/errors");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
/**
 * This class is like `ReadOnlyValidTransaction` but for
 * a transaction with an unknown tx type.
 */
class ReadOnlyValidUnknownTypeTransaction extends ethereumjs_tx_1.Transaction {
    static fromTxData(_txData, _opts) {
        throw new errors_1.InternalError("`fromTxData` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    static fromSerializedTx(_serialized, _opts) {
        throw new errors_1.InternalError("`fromSerializedTx` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    static fromRlpSerializedTx(_serialized, _opts) {
        throw new errors_1.InternalError("`fromRlpSerializedTx` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    static fromValuesArray(_values, _opts) {
        throw new errors_1.InternalError("`fromRlpSerializedTx` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    constructor(sender, type, data = {}) {
        super(data, { freeze: false, disableMaxInitCodeSizeCheck: true });
        this.common = this._getCommon();
        this._sender = sender;
        this._actualType = type;
    }
    get type() {
        return this._actualType;
    }
    verifySignature() {
        return true;
    }
    getSenderAddress() {
        return this._sender;
    }
    sign() {
        throw new errors_1.InternalError("`sign` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    getDataFee() {
        throw new errors_1.InternalError("`getDataFee` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    getBaseFee() {
        throw new errors_1.InternalError("`getBaseFee` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    getUpfrontCost() {
        throw new errors_1.InternalError("`getUpfrontCost` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    validate(_stringError = false) {
        throw new errors_1.InternalError("`validate` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    toCreationAddress() {
        throw new errors_1.InternalError("`toCreationAddress` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    getSenderPublicKey() {
        throw new errors_1.InternalError("`getSenderPublicKey` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    getMessageToVerifySignature() {
        throw new errors_1.InternalError("`getMessageToVerifySignature` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
    getMessageToSign() {
        throw new errors_1.InternalError("`getMessageToSign` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    }
}
exports.ReadOnlyValidUnknownTypeTransaction = ReadOnlyValidUnknownTypeTransaction;
// Override private methods
const ReadOnlyValidUnknownTypeTransactionPrototype = ReadOnlyValidUnknownTypeTransaction.prototype;
ReadOnlyValidUnknownTypeTransactionPrototype._validateTxV = function (_v, common) {
    return this._getCommon(common);
};
ReadOnlyValidUnknownTypeTransactionPrototype._signedTxImplementsEIP155 =
    function () {
        throw new errors_1.InternalError("`_signedTxImplementsEIP155` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    };
ReadOnlyValidUnknownTypeTransactionPrototype._unsignedTxImplementsEIP155 =
    function () {
        throw new errors_1.InternalError("`_unsignedTxImplementsEIP155` is not implemented in ReadOnlyValidUnknownTypeTransaction");
    };
ReadOnlyValidUnknownTypeTransactionPrototype._getMessageToSign = function () {
    throw new errors_1.InternalError("`_getMessageToSign` is not implemented in ReadOnlyValidUnknownTypeTransaction");
};
ReadOnlyValidUnknownTypeTransactionPrototype._processSignature = function () {
    throw new errors_1.InternalError("`_processSignature` is not implemented in ReadOnlyValidUnknownTypeTransaction");
};
//# sourceMappingURL=ReadOnlyValidUnknownTypeTransaction.js.map