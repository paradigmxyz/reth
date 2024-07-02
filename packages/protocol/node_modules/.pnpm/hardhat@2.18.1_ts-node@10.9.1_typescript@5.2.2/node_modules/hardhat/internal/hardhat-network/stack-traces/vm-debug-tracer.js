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
exports.VMDebugTracer = void 0;
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const errors_1 = require("../../core/errors");
const errors_2 = require("../../core/providers/errors");
const BigIntUtils = __importStar(require("../../util/bigint"));
function isStructLog(message) {
    return message !== undefined && !("structLogs" in message);
}
const EMPTY_MEMORY_WORD = "0".repeat(64);
class VMDebugTracer {
    constructor(_vm) {
        this._vm = _vm;
        this._messages = [];
        this._addressToStorage = {};
        this._beforeMessageHandler = this._beforeMessageHandler.bind(this);
        this._afterMessageHandler = this._afterMessageHandler.bind(this);
        this._beforeTxHandler = this._beforeTxHandler.bind(this);
        this._stepHandler = this._stepHandler.bind(this);
        this._afterTxHandler = this._afterTxHandler.bind(this);
    }
    /**
     * Run the `action` callback and trace its execution
     */
    async trace(action, config) {
        try {
            this._enableTracing(config);
            this._config = config;
            await action();
            if (this._error !== undefined) {
                throw this._error;
            }
            return this._getDebugTrace();
        }
        finally {
            this._disableTracing();
        }
    }
    _enableTracing(config) {
        (0, errors_1.assertHardhatInvariant)(this._vm.evm.events !== undefined, "EVM should have an 'events' property");
        this._vm.events.on("beforeTx", this._beforeTxHandler);
        this._vm.evm.events.on("beforeMessage", this._beforeMessageHandler);
        this._vm.evm.events.on("step", this._stepHandler);
        this._vm.evm.events.on("afterMessage", this._afterMessageHandler);
        this._vm.events.on("afterTx", this._afterTxHandler);
        this._config = config;
    }
    _disableTracing() {
        (0, errors_1.assertHardhatInvariant)(this._vm.evm.events !== undefined, "EVM should have an 'events' property");
        this._vm.events.removeListener("beforeTx", this._beforeTxHandler);
        this._vm.evm.events.removeListener("beforeMessage", this._beforeMessageHandler);
        this._vm.evm.events.removeListener("step", this._stepHandler);
        this._vm.evm.events.removeListener("afterMessage", this._afterMessageHandler);
        this._vm.events.removeListener("afterTx", this._afterTxHandler);
        this._config = undefined;
    }
    _getDebugTrace() {
        if (this._lastTrace === undefined) {
            throw new Error("No debug trace available. Please run the transaction first");
        }
        return this._lastTrace;
    }
    async _beforeTxHandler(_tx, next) {
        this._lastTrace = undefined;
        this._messages = [];
        this._addressToStorage = {};
        next();
    }
    async _beforeMessageHandler(message, next) {
        const debugMessage = {
            structLogs: [],
            to: message.to?.toString() ?? "",
        };
        if (this._messages.length > 0) {
            const previousMessage = this._messages[this._messages.length - 1];
            previousMessage.structLogs.push(debugMessage);
        }
        this._messages.push(debugMessage);
        next();
    }
    async _stepHandler(step, next) {
        try {
            (0, errors_1.assertHardhatInvariant)(this._messages.length > 0, "Step handler should be called after at least one beforeMessage handler");
            const structLog = await this._stepToStructLog(step);
            this._messages[this._messages.length - 1].structLogs.push(structLog);
        }
        catch (e) {
            // errors thrown in event handlers are lost, so we save this error to
            // re-throw it in the `trace` function
            this._error = e;
            this._disableTracing();
        }
        next();
    }
    async _afterMessageHandler(result, next) {
        const lastMessage = this._messages[this._messages.length - 1];
        lastMessage.result = result;
        if (this._messages.length > 1) {
            this._messages.pop();
        }
        next();
    }
    async _afterTxHandler(result, next) {
        const { default: flattenDeep } = await Promise.resolve().then(() => __importStar(require("lodash/flattenDeep")));
        const topLevelMessage = this._messages[0];
        const nestedStructLogs = await this._messageToNestedStructLogs(topLevelMessage, topLevelMessage.to);
        const rpcStructLogs = flattenDeep(nestedStructLogs).map((structLog) => {
            const rpcStructLog = structLog;
            // geth doesn't return this value
            delete rpcStructLog.memSize;
            if (this._config?.disableMemory === true) {
                delete rpcStructLog.memory;
            }
            if (this._config?.disableStack === true) {
                delete rpcStructLog.stack;
            }
            if (this._config?.disableStorage === true) {
                delete rpcStructLog.storage;
            }
            return rpcStructLog;
        });
        // geth does this for some reason
        if (rpcStructLogs.length > 0 &&
            result.execResult.exceptionError?.error === "out of gas") {
            rpcStructLogs[rpcStructLogs.length - 1].error = {};
        }
        this._lastTrace = {
            gas: Number(result.totalGasSpent),
            failed: result.execResult.exceptionError !== undefined,
            returnValue: result.execResult.returnValue.toString("hex"),
            structLogs: rpcStructLogs,
        };
        next();
    }
    async _messageToNestedStructLogs(message, address) {
        const nestedStructLogs = [];
        for (const [i, messageOrStructLog] of message.structLogs.entries()) {
            if (isStructLog(messageOrStructLog)) {
                const structLog = messageOrStructLog;
                nestedStructLogs.push(structLog);
                // update the storage of the current address
                const addressStorage = this._addressToStorage[address] ?? {};
                structLog.storage = {
                    ...addressStorage,
                    ...structLog.storage,
                };
                this._addressToStorage[address] = {
                    ...structLog.storage,
                };
                if (i === 0) {
                    continue;
                }
                let previousStructLog = nestedStructLogs[nestedStructLogs.length - 2];
                if (Array.isArray(previousStructLog)) {
                    previousStructLog = nestedStructLogs[nestedStructLogs.length - 3];
                }
                else {
                    // if the previous log is not a message, we update its gasCost
                    // using the gas difference between both steps
                    previousStructLog.gasCost = previousStructLog.gas - structLog.gas;
                }
                (0, errors_1.assertHardhatInvariant)(!Array.isArray(previousStructLog), "There shouldn't be two messages one after another");
                // memory opcodes reflect the expanded memory in that step,
                // so we correct them
                if (previousStructLog.op === "MSTORE" ||
                    previousStructLog.op === "MLOAD") {
                    const memoryLengthDifference = structLog.memory.length - previousStructLog.memory.length;
                    for (let k = 0; k < memoryLengthDifference; k++) {
                        previousStructLog.memory.push(EMPTY_MEMORY_WORD);
                    }
                }
            }
            else {
                const subMessage = messageOrStructLog;
                const lastStructLog = nestedStructLogs[nestedStructLogs.length - 1];
                (0, errors_1.assertHardhatInvariant)(!Array.isArray(lastStructLog), "There shouldn't be two messages one after another");
                const isDelegateCall = lastStructLog.op === "DELEGATECALL";
                const messageNestedStructLogs = await this._messageToNestedStructLogs(subMessage, isDelegateCall ? address : subMessage.to);
                nestedStructLogs.push(messageNestedStructLogs);
            }
        }
        return nestedStructLogs;
    }
    _getMemory(step) {
        const rawMemory = Buffer.from(step.memory)
            .toString("hex")
            .match(/.{1,64}/g) ?? [];
        // Remove the additional non allocated memory
        return rawMemory.slice(0, Number(step.memoryWordCount));
    }
    _getStack(step) {
        const stack = step.stack
            .slice()
            .map((el) => el.toString(16).padStart(64, "0"));
        return stack;
    }
    async _stepToStructLog(step) {
        const memory = this._getMemory(step);
        const stack = this._getStack(step);
        let gasCost = step.opcode.fee;
        let op = step.opcode.name;
        let error;
        const storage = {};
        if (step.opcode.name === "SLOAD") {
            const address = step.address;
            const [keyBuffer] = this._getFromStack(stack, 1);
            const key = (0, ethereumjs_util_1.setLengthLeft)(keyBuffer, 32);
            const storageValue = await this._getContractStorage(address, key);
            storage[toWord(key)] = toWord(storageValue);
        }
        else if (step.opcode.name === "SSTORE") {
            const [keyBuffer, valueBuffer] = this._getFromStack(stack, 2);
            const key = toWord(keyBuffer);
            const storageValue = toWord(valueBuffer);
            storage[key] = storageValue;
        }
        else if (step.opcode.name === "REVERT") {
            const [offsetBuffer, lengthBuffer] = this._getFromStack(stack, 2);
            const length = (0, ethereumjs_util_1.bufferToBigInt)(lengthBuffer);
            const offset = (0, ethereumjs_util_1.bufferToBigInt)(offsetBuffer);
            const [gasIncrease, addedWords] = this._memoryExpansion(BigInt(memory.length), length + offset);
            gasCost += Number(gasIncrease);
            for (let i = 0; i < addedWords; i++) {
                memory.push(EMPTY_MEMORY_WORD);
            }
        }
        else if (step.opcode.name === "CREATE2") {
            const [, , memoryUsedBuffer] = this._getFromStack(stack, 3);
            const memoryUsed = (0, ethereumjs_util_1.bufferToBigInt)(memoryUsedBuffer);
            const sha3ExtraCost = BigIntUtils.divUp(memoryUsed, 32n) * this._sha3WordGas();
            gasCost += Number(sha3ExtraCost);
        }
        else if (step.opcode.name === "CALL" ||
            step.opcode.name === "STATICCALL" ||
            step.opcode.name === "DELEGATECALL") {
            // this is a port of what geth does to compute the
            // gasCost of a *CALL step, with some simplifications
            // because we don't support pre-spuriousDragon hardforks
            let valueBuffer = Buffer.from([]);
            let [callCostBuffer, recipientAddressBuffer, inBuffer, inSizeBuffer, outBuffer, outSizeBuffer,] = this._getFromStack(stack, 6);
            // CALL has 7 parameters
            if (step.opcode.name === "CALL") {
                [
                    callCostBuffer,
                    recipientAddressBuffer,
                    valueBuffer,
                    inBuffer,
                    inSizeBuffer,
                    outBuffer,
                    outSizeBuffer,
                ] = this._getFromStack(stack, 7);
            }
            const callCost = (0, ethereumjs_util_1.bufferToBigInt)(callCostBuffer);
            const value = (0, ethereumjs_util_1.bufferToBigInt)(valueBuffer);
            const memoryLength = BigInt(memory.length);
            const inBN = (0, ethereumjs_util_1.bufferToBigInt)(inBuffer);
            const inSizeBN = (0, ethereumjs_util_1.bufferToBigInt)(inSizeBuffer);
            const inPosition = inSizeBN === 0n ? inSizeBN : inBN + inSizeBN;
            const outBN = (0, ethereumjs_util_1.bufferToBigInt)(outBuffer);
            const outSizeBN = (0, ethereumjs_util_1.bufferToBigInt)(outSizeBuffer);
            const outPosition = outSizeBN === 0n ? outSizeBN : outBN + outSizeBN;
            const memSize = inPosition > outPosition ? inPosition : outPosition;
            const toAddress = new ethereumjs_util_1.Address(recipientAddressBuffer.slice(-20));
            const constantGas = this._callConstantGas();
            const availableGas = step.gasLeft - constantGas;
            const [memoryGas] = this._memoryExpansion(memoryLength, memSize);
            const dynamicGas = await this._callDynamicGas(toAddress, value, availableGas, memoryGas, callCost);
            gasCost = Number(constantGas + dynamicGas);
        }
        else if (step.opcode.name === "CALLCODE") {
            // finding an existing tx that uses CALLCODE or compiling a contract
            // so that it uses this opcode is hard,
            // so we just throw
            throw new errors_2.InvalidInputError("Transactions that use CALLCODE are not supported by Hardhat's debug_traceTransaction");
        }
        else if (step.opcode.name === "INVALID") {
            const code = await this._getContractCode(step.codeAddress);
            const opcodeHex = code[step.pc].toString(16);
            op = `opcode 0x${opcodeHex} not defined`;
            error = {};
        }
        const structLog = {
            pc: step.pc,
            op,
            gas: Number(step.gasLeft),
            gasCost,
            depth: step.depth + 1,
            stack,
            memory,
            storage,
            memSize: Number(step.memoryWordCount),
        };
        if (error !== undefined) {
            structLog.error = error;
        }
        return structLog;
    }
    _memoryGas() {
        return this._vm._common.param("gasPrices", "memory");
    }
    _sha3WordGas() {
        return this._vm._common.param("gasPrices", "sha3Word");
    }
    _callConstantGas() {
        if (this._vm._common.gteHardfork("berlin")) {
            return this._vm._common.param("gasPrices", "warmstorageread");
        }
        return this._vm._common.param("gasPrices", "call");
    }
    _callNewAccountGas() {
        return this._vm._common.param("gasPrices", "callNewAccount");
    }
    _callValueTransferGas() {
        return this._vm._common.param("gasPrices", "callValueTransfer");
    }
    _quadCoeffDiv() {
        return this._vm._common.param("gasPrices", "quadCoeffDiv");
    }
    _isAddressEmpty(address) {
        return this._vm.stateManager.accountIsEmpty(address);
    }
    _getContractStorage(address, key) {
        return this._vm.stateManager.getContractStorage(address, key);
    }
    _getContractCode(address) {
        return this._vm.stateManager.getContractCode(address);
    }
    async _callDynamicGas(address, value, availableGas, memoryGas, callCost) {
        // The available gas is reduced when the address is cold
        if (this._vm._common.gteHardfork("berlin")) {
            const isWarmed = this._vm.eei.isWarmedAddress(address.toBuffer());
            const coldCost = this._vm._common.param("gasPrices", "coldaccountaccess") -
                this._vm._common.param("gasPrices", "warmstorageread");
            // This comment is copied verbatim from geth:
            // The WarmStorageReadCostEIP2929 (100) is already deducted in the form of a constant cost, so
            // the cost to charge for cold access, if any, is Cold - Warm
            if (!isWarmed) {
                availableGas -= coldCost;
            }
        }
        let gas = 0n;
        const transfersValue = value !== 0n;
        const addressIsEmpty = await this._isAddressEmpty(address);
        if (transfersValue && addressIsEmpty) {
            gas += this._callNewAccountGas();
        }
        if (transfersValue) {
            gas += this._callValueTransferGas();
        }
        gas += memoryGas;
        gas += this._callGas(availableGas, gas, callCost);
        return gas;
    }
    _callGas(availableGas, base, callCost) {
        availableGas -= base;
        const gas = availableGas - availableGas / 64n;
        if (callCost > gas) {
            return gas;
        }
        return callCost;
    }
    /**
     * Returns the increase in gas and the number of added words
     */
    _memoryExpansion(currentWords, newSize) {
        const currentSize = currentWords * 32n;
        const currentWordsLength = (currentSize + 31n) / 32n;
        const newWordsLength = (newSize + 31n) / 32n;
        const wordsDiff = newWordsLength - currentWordsLength;
        if (newSize > currentSize) {
            const newTotalFee = this._memoryFee(newWordsLength);
            const currentTotalFee = this._memoryFee(currentWordsLength);
            const fee = newTotalFee - currentTotalFee;
            return [fee, wordsDiff];
        }
        return [0n, 0n];
    }
    _getFromStack(stack, count) {
        return stack
            .slice(-count)
            .reverse()
            .map((value) => `0x${value}`)
            .map(ethereumjs_util_1.toBuffer);
    }
    _memoryFee(words) {
        const square = words * words;
        const linCoef = words * this._memoryGas();
        const quadCoef = square / this._quadCoeffDiv();
        const newTotalFee = linCoef + quadCoef;
        return newTotalFee;
    }
}
exports.VMDebugTracer = VMDebugTracer;
function toWord(b) {
    return b.toString("hex").padStart(64, "0");
}
//# sourceMappingURL=vm-debug-tracer.js.map