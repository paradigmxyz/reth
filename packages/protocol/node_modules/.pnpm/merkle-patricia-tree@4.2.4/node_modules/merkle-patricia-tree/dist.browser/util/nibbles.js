"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.doKeysMatch = exports.matchingNibbleLength = exports.nibblesCompare = exports.nibblesToBuffer = exports.bufferToNibbles = void 0;
/**
 * Converts a buffer to a nibble array.
 * @private
 * @param key
 */
function bufferToNibbles(key) {
    var bkey = Buffer.from(key);
    var nibbles = [];
    for (var i = 0; i < bkey.length; i++) {
        var q = i * 2;
        nibbles[q] = bkey[i] >> 4;
        ++q;
        nibbles[q] = bkey[i] % 16;
    }
    return nibbles;
}
exports.bufferToNibbles = bufferToNibbles;
/**
 * Converts a nibble array into a buffer.
 * @private
 * @param arr - Nibble array
 */
function nibblesToBuffer(arr) {
    var buf = Buffer.alloc(arr.length / 2);
    for (var i = 0; i < buf.length; i++) {
        var q = i * 2;
        buf[i] = (arr[q] << 4) + arr[++q];
    }
    return buf;
}
exports.nibblesToBuffer = nibblesToBuffer;
/**
 * Compare two nibble array.
 * * `0` is returned if `n2` == `n1`.
 * * `1` is returned if `n2` > `n1`.
 * * `-1` is returned if `n2` < `n1`.
 * @param n1 - Nibble array
 * @param n2 - Nibble array
 */
function nibblesCompare(n1, n2) {
    var cmpLength = Math.min(n1.length, n2.length);
    var res = 0;
    for (var i = 0; i < cmpLength; i++) {
        if (n1[i] < n2[i]) {
            res = -1;
            break;
        }
        else if (n1[i] > n2[i]) {
            res = 1;
            break;
        }
    }
    if (res === 0) {
        if (n1.length < n2.length) {
            res = -1;
        }
        else if (n1.length > n2.length) {
            res = 1;
        }
    }
    return res;
}
exports.nibblesCompare = nibblesCompare;
/**
 * Returns the number of in order matching nibbles of two give nibble arrays.
 * @private
 * @param nib1
 * @param nib2
 */
function matchingNibbleLength(nib1, nib2) {
    var i = 0;
    while (nib1[i] === nib2[i] && nib1.length > i) {
        i++;
    }
    return i;
}
exports.matchingNibbleLength = matchingNibbleLength;
/**
 * Compare two nibble array keys.
 * @param keyA
 * @param keyB
 */
function doKeysMatch(keyA, keyB) {
    var length = matchingNibbleLength(keyA, keyB);
    return length === keyA.length && length === keyB.length;
}
exports.doKeysMatch = doKeysMatch;
//# sourceMappingURL=nibbles.js.map