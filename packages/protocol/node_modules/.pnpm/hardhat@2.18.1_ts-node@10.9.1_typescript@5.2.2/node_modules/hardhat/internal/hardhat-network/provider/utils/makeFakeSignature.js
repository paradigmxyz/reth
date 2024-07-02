"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.makeFakeSignature = void 0;
const hash_1 = require("../../../util/hash");
// Produces a signature with r and s values taken from a hash of the inputs.
function makeFakeSignature(tx, sender) {
    const hashInputString = [
        sender,
        tx.nonce,
        tx.gasLimit,
        tx.value,
        tx.to,
        tx.data,
        "gasPrice" in tx ? tx.gasPrice : "",
        "chainId" in tx ? tx.chainId : "",
        "maxPriorityFeePerGas" in tx ? tx.maxPriorityFeePerGas : "",
        "maxFeePerGas" in tx ? tx.maxFeePerGas : "",
        "accessList" in tx
            ? tx.accessList?.map(([buf, bufs]) => [buf, ...bufs].map((b) => b.toString("hex")).join(";"))
            : "",
    ]
        .map((a) => a?.toString() ?? "")
        .join(",");
    const hashDigest = (0, hash_1.createNonCryptographicHashBasedIdentifier)(Buffer.from(hashInputString));
    return {
        r: hashDigest.readUInt32LE(),
        s: hashDigest.readUInt32LE(4),
    };
}
exports.makeFakeSignature = makeFakeSignature;
//# sourceMappingURL=makeFakeSignature.js.map