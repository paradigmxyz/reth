"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ReturnData = void 0;
const errors_1 = require("../../core/errors");
const { rawDecode } = require("ethereumjs-abi");
// selector of Error(string)
const ERROR_SELECTOR = "08c379a0";
// selector of Panic(uint256)
const PANIC_SELECTOR = "4e487b71";
/**
 * Represents the returnData of a transaction, whose contents are unknown.
 */
class ReturnData {
    constructor(value) {
        this.value = value;
        if (value.length >= 4) {
            this._selector = value.slice(0, 4).toString("hex");
        }
    }
    isEmpty() {
        return this.value.length === 0;
    }
    matchesSelector(selector) {
        if (this._selector === undefined) {
            return false;
        }
        return this._selector === selector.toString("hex");
    }
    isErrorReturnData() {
        return this._selector === ERROR_SELECTOR;
    }
    isPanicReturnData() {
        return this._selector === PANIC_SELECTOR;
    }
    decodeError() {
        if (this.isEmpty()) {
            return "";
        }
        (0, errors_1.assertHardhatInvariant)(this._selector === ERROR_SELECTOR, "Expected return data to be a Error(string)");
        const [decodedError] = rawDecode(["string"], this.value.slice(4));
        return decodedError;
    }
    decodePanic() {
        (0, errors_1.assertHardhatInvariant)(this._selector === PANIC_SELECTOR, "Expected return data to be a Panic(uint256)");
        // we are assuming that panic codes are smaller than Number.MAX_SAFE_INTEGER
        const errorCode = BigInt(`0x${this.value.slice(4).toString("hex")}`);
        return errorCode;
    }
    getSelector() {
        return this._selector;
    }
}
exports.ReturnData = ReturnData;
//# sourceMappingURL=return-data.js.map