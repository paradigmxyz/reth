"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.gindexChild = exports.gindexParent = exports.gindexSibling = exports.concatGindices = exports.getGindexBits = exports.gindexIterator = exports.getGindicesAtDepth = exports.iterateAtDepth = exports.countToDepth = exports.convertGindexToBitstring = exports.toGindexBitstring = exports.toGindex = exports.bitIndexBigInt = void 0;
function bitIndexBigInt(v) {
    return v.toString(2).length - 1;
}
exports.bitIndexBigInt = bitIndexBigInt;
function toGindex(depth, index) {
    const anchor = BigInt(1) << BigInt(depth);
    if (index >= anchor) {
        throw new Error(`index ${index} too large for depth ${depth}`);
    }
    return anchor | index;
}
exports.toGindex = toGindex;
function toGindexBitstring(depth, index) {
    const str = index ? Number(index).toString(2) : "";
    if (str.length > depth) {
        throw new Error("index too large for depth");
    }
    else {
        return "1" + str.padStart(depth, "0");
    }
}
exports.toGindexBitstring = toGindexBitstring;
function convertGindexToBitstring(gindex) {
    if (typeof gindex === "string") {
        if (gindex.length === 0) {
            throw new Error(ERR_INVALID_GINDEX);
        }
        return gindex;
    }
    else {
        if (gindex < 1) {
            throw new Error(ERR_INVALID_GINDEX);
        }
        return gindex.toString(2);
    }
}
exports.convertGindexToBitstring = convertGindexToBitstring;
// Get the depth (root starting at 0) necessary to cover a subtree of `count` elements.
// (in out): (0 0), (1 0), (2 1), (3 2), (4 2), (5 3), (6 3), (7 3), (8 3), (9 4)
function countToDepth(count) {
    if (count <= 1) {
        return 0;
    }
    return (count - BigInt(1)).toString(2).length;
}
exports.countToDepth = countToDepth;
/**
 * Iterate through Gindexes at a certain depth
 */
function iterateAtDepth(depth, startIndex, count) {
    const anchor = BigInt(1) << BigInt(depth);
    if (startIndex + count > anchor) {
        throw new Error("Too large for depth");
    }
    let i = toGindex(depth, startIndex);
    const last = i + count;
    return {
        [Symbol.iterator]() {
            return {
                next() {
                    if (i < last) {
                        const value = i;
                        i++;
                        return { done: false, value };
                    }
                    else {
                        return { done: true, value: undefined };
                    }
                },
            };
        },
    };
}
exports.iterateAtDepth = iterateAtDepth;
/**
 * Return Gindexes at a certain depth
 */
function getGindicesAtDepth(depth, startIndex, count) {
    const anchor = BigInt(1) << BigInt(depth);
    if (startIndex + count > anchor) {
        throw new Error("Too large for depth");
    }
    let gindex = toGindex(depth, BigInt(startIndex));
    const gindices = [];
    for (let i = 0; i < count; i++) {
        gindices.push(gindex++);
    }
    return gindices;
}
exports.getGindicesAtDepth = getGindicesAtDepth;
const ERR_INVALID_GINDEX = "Invalid gindex";
function gindexIterator(gindex) {
    let bitstring;
    if (typeof gindex === "string") {
        if (!gindex.length) {
            throw new Error(ERR_INVALID_GINDEX);
        }
        bitstring = gindex;
    }
    else {
        if (gindex < 1) {
            throw new Error(ERR_INVALID_GINDEX);
        }
        bitstring = gindex.toString(2);
    }
    let i = 1;
    const next = () => {
        if (i === bitstring.length) {
            return { done: true, value: undefined };
        }
        const bit = Number(bitstring[i]);
        i++;
        return { done: false, value: bit };
    };
    return {
        [Symbol.iterator]() {
            return { next };
        },
        remainingBitLength() {
            return bitstring.length - i;
        },
    };
}
exports.gindexIterator = gindexIterator;
function getGindexBits(gindex) {
    let bitstring;
    if (typeof gindex === "string") {
        if (!gindex.length) {
            throw new Error(ERR_INVALID_GINDEX);
        }
        bitstring = gindex;
    }
    else {
        if (gindex < 1) {
            throw new Error(ERR_INVALID_GINDEX);
        }
        bitstring = gindex.toString(2);
    }
    const bits = [];
    for (let i = 1; i < bitstring.length; i++) {
        bits.push(Number(bitstring[i]));
    }
    return bits;
}
exports.getGindexBits = getGindexBits;
/**
 * Concatenate Generalized Indices
 * Given generalized indices i1 for A -> B, i2 for B -> C .... i_n for Y -> Z, returns
 * the generalized index for A -> Z.
 */
function concatGindices(gindices) {
    return BigInt(gindices.reduce((acc, gindex) => acc + gindex.toString(2).slice(1), "0b1"));
}
exports.concatGindices = concatGindices;
function gindexSibling(gindex) {
    return gindex ^ BigInt(1);
}
exports.gindexSibling = gindexSibling;
function gindexParent(gindex) {
    return gindex / BigInt(2);
}
exports.gindexParent = gindexParent;
function gindexChild(gindex, rightChild) {
    return gindex * BigInt(2) + BigInt(rightChild);
}
exports.gindexChild = gindexChild;
