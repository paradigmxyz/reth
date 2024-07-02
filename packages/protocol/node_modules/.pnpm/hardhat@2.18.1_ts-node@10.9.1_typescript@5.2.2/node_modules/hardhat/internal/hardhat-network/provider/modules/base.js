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
exports.Base = void 0;
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const BigIntUtils = __importStar(require("../../../util/bigint"));
const errors_1 = require("../../../core/providers/errors");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
class Base {
    constructor(_node) {
        this._node = _node;
    }
    async resolveNewBlockTag(newBlockTag, defaultValue = "latest") {
        if (newBlockTag === undefined) {
            newBlockTag = defaultValue;
        }
        if (newBlockTag === "pending") {
            return "pending";
        }
        if (newBlockTag === "latest") {
            return this._node.getLatestBlockNumber();
        }
        if (newBlockTag === "earliest") {
            return 0n;
        }
        if (newBlockTag === "safe" || newBlockTag === "finalized") {
            this._checkPostMergeBlockTags(newBlockTag);
            return this._node.getLatestBlockNumber();
        }
        if (!BigIntUtils.isBigInt(newBlockTag)) {
            if ("blockNumber" in newBlockTag && "blockHash" in newBlockTag) {
                throw new errors_1.InvalidArgumentsError("Invalid block tag received. Only one of hash or block number can be used.");
            }
            if ("blockNumber" in newBlockTag && "requireCanonical" in newBlockTag) {
                throw new errors_1.InvalidArgumentsError("Invalid block tag received. requireCanonical only works with hashes.");
            }
        }
        let block;
        if (BigIntUtils.isBigInt(newBlockTag)) {
            block = await this._node.getBlockByNumber(newBlockTag);
        }
        else if ("blockNumber" in newBlockTag) {
            block = await this._node.getBlockByNumber(newBlockTag.blockNumber);
        }
        else {
            block = await this._node.getBlockByHash(newBlockTag.blockHash);
        }
        if (block === undefined) {
            const latestBlock = this._node.getLatestBlockNumber();
            throw new errors_1.InvalidInputError(`Received invalid block tag ${this._newBlockTagToString(newBlockTag)}. Latest block number is ${latestBlock.toString()}`);
        }
        return block.header.number;
    }
    async rpcCallRequestToNodeCallParams(rpcCall) {
        return {
            to: rpcCall.to,
            from: rpcCall.from !== undefined
                ? rpcCall.from
                : await this._getDefaultCallFrom(),
            data: rpcCall.data !== undefined ? rpcCall.data : (0, ethereumjs_util_1.toBuffer)([]),
            gasLimit: rpcCall.gas !== undefined ? rpcCall.gas : this._node.getBlockGasLimit(),
            value: rpcCall.value !== undefined ? rpcCall.value : 0n,
            accessList: rpcCall.accessList !== undefined
                ? this._rpcAccessListToNodeAccessList(rpcCall.accessList)
                : undefined,
            gasPrice: rpcCall.gasPrice,
            maxFeePerGas: rpcCall.maxFeePerGas,
            maxPriorityFeePerGas: rpcCall.maxPriorityFeePerGas,
        };
    }
    _rpcAccessListToNodeAccessList(rpcAccessList) {
        return rpcAccessList.map((tuple) => [
            tuple.address,
            tuple.storageKeys ?? [],
        ]);
    }
    _checkPostMergeBlockTags(blockTag) {
        const isPostMerge = this._node.isPostMergeHardfork();
        const hardfork = this._node.hardfork;
        if (!isPostMerge) {
            throw new errors_1.InvalidArgumentsError(`The '${blockTag}' block tag is not allowed in pre-merge hardforks. You are using the '${hardfork}' hardfork.`);
        }
    }
    _newBlockTagToString(tag) {
        if (typeof tag === "string") {
            return tag;
        }
        if (BigIntUtils.isBigInt(tag)) {
            return tag.toString();
        }
        if ("blockNumber" in tag) {
            return tag.blockNumber.toString();
        }
        return (0, ethereumjs_util_1.bufferToHex)(tag.blockHash);
    }
    async _getDefaultCallFrom() {
        const localAccounts = await this._node.getLocalAccountAddresses();
        if (localAccounts.length === 0) {
            return (0, ethereumjs_util_1.toBuffer)((0, ethereumjs_util_1.zeroAddress)());
        }
        return (0, ethereumjs_util_1.toBuffer)(localAccounts[0]);
    }
}
exports.Base = Base;
//# sourceMappingURL=base.js.map