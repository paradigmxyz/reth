"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.isHashObject = exports.uint8ArrayToHashObject = exports.hashObjectToUint8Array = exports.hashTwoObjects = exports.hash = void 0;
const as_sha256_1 = require("@chainsafe/as-sha256");
const input = new Uint8Array(64);
/**
 * Hash two 32 byte arrays
 */
function hash(a, b) {
    input.set(a, 0);
    input.set(b, 32);
    return as_sha256_1.digest64(input);
}
exports.hash = hash;
/**
 * Hash 2 objects, each store 8 numbers (equivalent to Uint8Array(32))
 */
function hashTwoObjects(a, b) {
    return as_sha256_1.digest64HashObjects(a, b);
}
exports.hashTwoObjects = hashTwoObjects;
function hashObjectToUint8Array(obj) {
    const byteArr = new Uint8Array(32);
    as_sha256_1.hashObjectToByteArray(obj, byteArr, 0);
    return byteArr;
}
exports.hashObjectToUint8Array = hashObjectToUint8Array;
function uint8ArrayToHashObject(byteArr) {
    return as_sha256_1.byteArrayToHashObject(byteArr);
}
exports.uint8ArrayToHashObject = uint8ArrayToHashObject;
function isHashObject(hash) {
    // @ts-ignore
    return hash.length === undefined;
}
exports.isHashObject = isHashObject;
