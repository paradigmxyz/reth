"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.Web3Module = void 0;
const base_types_1 = require("../../../core/jsonrpc/types/base-types");
const validation_1 = require("../../../core/jsonrpc/types/input/validation");
const errors_1 = require("../../../core/providers/errors");
const keccak_1 = require("../../../util/keccak");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
class Web3Module {
    constructor(_node) {
        this._node = _node;
    }
    async processRequest(method, params = []) {
        switch (method) {
            case "web3_clientVersion":
                return this._clientVersionAction(...this._clientVersionParams(params));
            case "web3_sha3":
                return this._sha3Action(...this._sha3Params(params));
        }
        throw new errors_1.MethodNotFoundError(`Method ${method} not found`);
    }
    // web3_clientVersion
    _clientVersionParams(params) {
        return (0, validation_1.validateParams)(params);
    }
    async _clientVersionAction() {
        return this._node.getClientVersion();
    }
    // web3_sha3
    _sha3Params(params) {
        return (0, validation_1.validateParams)(params, base_types_1.rpcData);
    }
    async _sha3Action(buffer) {
        return (0, base_types_1.bufferToRpcData)((0, keccak_1.keccak256)(buffer));
    }
}
exports.Web3Module = Web3Module;
//# sourceMappingURL=web3.js.map