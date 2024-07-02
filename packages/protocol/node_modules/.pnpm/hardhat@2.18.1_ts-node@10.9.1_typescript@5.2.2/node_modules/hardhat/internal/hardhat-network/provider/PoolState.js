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
exports.makePoolState = exports.makeSerializedTransaction = void 0;
const immutable_1 = require("immutable");
const BigIntUtils = __importStar(require("../../util/bigint"));
exports.makeSerializedTransaction = (0, immutable_1.Record)({
    orderId: 0,
    fakeFrom: undefined,
    data: "",
    txType: 0,
});
exports.makePoolState = (0, immutable_1.Record)({
    pendingTransactions: (0, immutable_1.Map)(),
    queuedTransactions: (0, immutable_1.Map)(),
    hashToTransaction: (0, immutable_1.Map)(),
    blockGasLimit: BigIntUtils.toHex(9500000),
});
//# sourceMappingURL=PoolState.js.map