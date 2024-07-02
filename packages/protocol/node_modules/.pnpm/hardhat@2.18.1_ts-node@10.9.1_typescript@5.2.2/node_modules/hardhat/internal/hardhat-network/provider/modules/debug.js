"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.DebugModule = void 0;
const base_types_1 = require("../../../core/jsonrpc/types/base-types");
const blockTag_1 = require("../../../core/jsonrpc/types/input/blockTag");
const callRequest_1 = require("../../../core/jsonrpc/types/input/callRequest");
const debugTraceTransaction_1 = require("../../../core/jsonrpc/types/input/debugTraceTransaction");
const validation_1 = require("../../../core/jsonrpc/types/input/validation");
const errors_1 = require("../../../core/providers/errors");
const base_1 = require("./base");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
class DebugModule extends base_1.Base {
    constructor(_node) {
        super(_node);
    }
    async processRequest(method, params = []) {
        switch (method) {
            case "debug_traceCall":
                return this._traceCallAction(...this._traceCallParams(params));
            case "debug_traceTransaction":
                return this._traceTransactionAction(...this._traceTransactionParams(params));
        }
        throw new errors_1.MethodNotFoundError(`Method ${method} not found`);
    }
    // debug_traceCall
    _traceCallParams(params) {
        const validatedParams = (0, validation_1.validateParams)(params, callRequest_1.rpcCallRequest, blockTag_1.optionalRpcNewBlockTag, debugTraceTransaction_1.rpcDebugTracingConfig);
        this._validateTracerParam(validatedParams[2]);
        return validatedParams;
    }
    async _traceCallAction(callConfig, block, traceConfig) {
        const callParams = await this.rpcCallRequestToNodeCallParams(callConfig);
        const blockNumber = await this.resolveNewBlockTag(block);
        return this._node.traceCall(callParams, blockNumber, traceConfig);
    }
    // debug_traceTransaction
    _traceTransactionParams(params) {
        const validatedParams = (0, validation_1.validateParams)(params, base_types_1.rpcHash, debugTraceTransaction_1.rpcDebugTracingConfig);
        this._validateTracerParam(validatedParams[1]);
        return validatedParams;
    }
    async _traceTransactionAction(hash, config) {
        return this._node.traceTransaction(hash, config);
    }
    _validateTracerParam(config) {
        if (config?.tracer !== undefined) {
            throw new errors_1.InvalidArgumentsError("Hardhat currently only supports the default tracer, so no tracer parameter should be passed.");
        }
    }
}
exports.DebugModule = DebugModule;
//# sourceMappingURL=debug.js.map