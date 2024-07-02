"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.getCurrentTimestampBigInt = exports.getCurrentTimestamp = void 0;
function getCurrentTimestamp() {
    return Math.ceil(new Date().getTime() / 1000);
}
exports.getCurrentTimestamp = getCurrentTimestamp;
function getCurrentTimestampBigInt() {
    return BigInt(getCurrentTimestamp());
}
exports.getCurrentTimestampBigInt = getCurrentTimestampBigInt;
//# sourceMappingURL=getCurrentTimestamp.js.map