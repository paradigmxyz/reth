"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.byteArrayEquals = exports.fromHexString = exports.toHexString = void 0;
// Caching this info costs about ~1000 bytes and speeds up toHexString() by x6
const hexByByte = new Array(256);
function toHexString(bytes) {
    let hex = "0x";
    for (const byte of bytes) {
        if (!hexByByte[byte]) {
            hexByByte[byte] = byte < 16 ? "0" + byte.toString(16) : byte.toString(16);
        }
        hex += hexByByte[byte];
    }
    return hex;
}
exports.toHexString = toHexString;
function fromHexString(hex) {
    if (typeof hex !== "string") {
        throw new Error(`hex argument type ${typeof hex} must be of type string`);
    }
    if (hex.startsWith("0x")) {
        hex = hex.slice(2);
    }
    if (hex.length % 2 !== 0) {
        throw new Error(`hex string length ${hex.length} must be multiple of 2`);
    }
    const byteLen = hex.length / 2;
    const bytes = new Uint8Array(byteLen);
    for (let i = 0; i < byteLen; i++) {
        const byte = parseInt(hex.slice(i * 2, (i + 1) * 2), 16);
        bytes[i] = byte;
    }
    return bytes;
}
exports.fromHexString = fromHexString;
function byteArrayEquals(a, b) {
    if (a.length !== b.length) {
        return false;
    }
    for (let i = 0; i < a.length; i++) {
        if (a[i] !== b[i])
            return false;
    }
    return true;
}
exports.byteArrayEquals = byteArrayEquals;
//# sourceMappingURL=byteArray.js.map