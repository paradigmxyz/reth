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
exports.reorganizeTransactionsLists = void 0;
const errors_1 = require("../../../core/providers/errors");
const BigIntUtils = __importStar(require("../../../util/bigint"));
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
/**
 * Move as many transactions as possible from the queued list
 * to the pending list.
 *
 * Returns the new lists and the new executable nonce of the sender.
 */
function reorganizeTransactionsLists(pending, queued, retrieveNonce) {
    let newPending = pending;
    let newQueued = queued.sortBy(retrieveNonce, (l, r) => BigIntUtils.cmp(l, r));
    if (pending.last() === undefined) {
        throw new errors_1.InternalError("Pending list cannot be empty");
    }
    let nextPendingNonce = retrieveNonce(pending.last()) + 1n;
    let movedCount = 0;
    for (const queuedTx of newQueued) {
        const queuedTxNonce = retrieveNonce(queuedTx);
        if (nextPendingNonce === queuedTxNonce) {
            newPending = newPending.push(queuedTx);
            nextPendingNonce++;
            movedCount++;
        }
        else {
            break;
        }
    }
    newQueued = newQueued.skip(movedCount);
    return {
        newPending,
        newQueued,
    };
}
exports.reorganizeTransactionsLists = reorganizeTransactionsLists;
//# sourceMappingURL=reorganizeTransactionsLists.js.map