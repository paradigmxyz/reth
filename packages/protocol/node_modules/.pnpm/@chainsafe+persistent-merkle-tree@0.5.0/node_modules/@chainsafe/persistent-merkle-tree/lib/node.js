"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.bitwiseOrNodeH = exports.setNodeH = exports.getNodeH = exports.compose = exports.identity = exports.LeafNode = exports.BranchNode = exports.Node = void 0;
const hash_1 = require("./hash");
const TWO_POWER_32 = 2 ** 32;
/**
 * An immutable binary merkle tree node
 */
class Node {
    constructor(h0, h1, h2, h3, h4, h5, h6, h7) {
        this.h0 = h0;
        this.h1 = h1;
        this.h2 = h2;
        this.h3 = h3;
        this.h4 = h4;
        this.h5 = h5;
        this.h6 = h6;
        this.h7 = h7;
    }
    applyHash(root) {
        this.h0 = root.h0;
        this.h1 = root.h1;
        this.h2 = root.h2;
        this.h3 = root.h3;
        this.h4 = root.h4;
        this.h5 = root.h5;
        this.h6 = root.h6;
        this.h7 = root.h7;
    }
}
exports.Node = Node;
/**
 * An immutable binary merkle tree node that has a `left` and `right` child
 */
class BranchNode extends Node {
    constructor(_left, _right) {
        // First null value is to save an extra variable to check if a node has a root or not
        super(null, 0, 0, 0, 0, 0, 0, 0);
        this._left = _left;
        this._right = _right;
        if (!_left) {
            throw new Error("Left node is undefined");
        }
        if (!_right) {
            throw new Error("Right node is undefined");
        }
    }
    get rootHashObject() {
        if (this.h0 === null) {
            super.applyHash(hash_1.hashTwoObjects(this.left.rootHashObject, this.right.rootHashObject));
        }
        return this;
    }
    get root() {
        return hash_1.hashObjectToUint8Array(this.rootHashObject);
    }
    isLeaf() {
        return false;
    }
    get left() {
        return this._left;
    }
    get right() {
        return this._right;
    }
}
exports.BranchNode = BranchNode;
/**
 * An immutable binary merkle tree node that has no children
 */
class LeafNode extends Node {
    static fromRoot(root) {
        return this.fromHashObject(hash_1.uint8ArrayToHashObject(root));
    }
    /**
     * New LeafNode from existing HashObject.
     */
    static fromHashObject(ho) {
        return new LeafNode(ho.h0, ho.h1, ho.h2, ho.h3, ho.h4, ho.h5, ho.h6, ho.h7);
    }
    /**
     * New LeafNode with its internal value set to zero. Consider using `zeroNode(0)` if you don't need to mutate.
     */
    static fromZero() {
        return new LeafNode(0, 0, 0, 0, 0, 0, 0, 0);
    }
    /**
     * LeafNode with HashObject `(uint32, 0, 0, 0, 0, 0, 0, 0)`.
     */
    static fromUint32(uint32) {
        return new LeafNode(uint32, 0, 0, 0, 0, 0, 0, 0);
    }
    /**
     * Create a new LeafNode with the same internal values. The returned instance is safe to mutate
     */
    clone() {
        return LeafNode.fromHashObject(this);
    }
    get rootHashObject() {
        return this;
    }
    get root() {
        return hash_1.hashObjectToUint8Array(this);
    }
    isLeaf() {
        return true;
    }
    get left() {
        throw Error("LeafNode has no left node");
    }
    get right() {
        throw Error("LeafNode has no right node");
    }
    writeToBytes(data, start, size) {
        // TODO: Optimize
        data.set(this.root.slice(0, size), start);
    }
    getUint(uintBytes, offsetBytes, clipInfinity) {
        const hIndex = Math.floor(offsetBytes / 4);
        // number has to be masked from an h value
        if (uintBytes < 4) {
            const bitIndex = (offsetBytes % 4) * 8;
            const h = getNodeH(this, hIndex);
            if (uintBytes === 1) {
                return 0xff & (h >> bitIndex);
            }
            else {
                return 0xffff & (h >> bitIndex);
            }
        }
        // number equals the h value
        else if (uintBytes === 4) {
            return getNodeH(this, hIndex) >>> 0;
        }
        // number spans 2 h values
        else if (uintBytes === 8) {
            const low = getNodeH(this, hIndex);
            const high = getNodeH(this, hIndex + 1);
            if (high === 0) {
                return low >>> 0;
            }
            else if (high === -1 && low === -1 && clipInfinity) {
                // Limit uint returns
                return Infinity;
            }
            else {
                return (low >>> 0) + (high >>> 0) * TWO_POWER_32;
            }
        }
        // Bigger uint can't be represented
        else {
            throw Error("uintBytes > 8");
        }
    }
    getUintBigint(uintBytes, offsetBytes) {
        const hIndex = Math.floor(offsetBytes / 4);
        // number has to be masked from an h value
        if (uintBytes < 4) {
            const bitIndex = (offsetBytes % 4) * 8;
            const h = getNodeH(this, hIndex);
            if (uintBytes === 1) {
                return BigInt(0xff & (h >> bitIndex));
            }
            else {
                return BigInt(0xffff & (h >> bitIndex));
            }
        }
        // number equals the h value
        else if (uintBytes === 4) {
            return BigInt(getNodeH(this, hIndex) >>> 0);
        }
        // number spans multiple h values
        else {
            const hRange = Math.ceil(uintBytes / 4);
            let v = BigInt(0);
            for (let i = 0; i < hRange; i++) {
                v += BigInt(getNodeH(this, hIndex + i) >>> 0) << BigInt(32 * i);
            }
            return v;
        }
    }
    setUint(uintBytes, offsetBytes, value, clipInfinity) {
        const hIndex = Math.floor(offsetBytes / 4);
        // number has to be masked from an h value
        if (uintBytes < 4) {
            const bitIndex = (offsetBytes % 4) * 8;
            let h = getNodeH(this, hIndex);
            if (uintBytes === 1) {
                h &= ~(0xff << bitIndex);
                h |= (0xff && value) << bitIndex;
            }
            else {
                h &= ~(0xffff << bitIndex);
                h |= (0xffff && value) << bitIndex;
            }
            setNodeH(this, hIndex, h);
        }
        // number equals the h value
        else if (uintBytes === 4) {
            setNodeH(this, hIndex, value);
        }
        // number spans 2 h values
        else if (uintBytes === 8) {
            if (value === Infinity && clipInfinity) {
                setNodeH(this, hIndex, -1);
                setNodeH(this, hIndex + 1, -1);
            }
            else {
                setNodeH(this, hIndex, value & 0xffffffff);
                setNodeH(this, hIndex + 1, (value / TWO_POWER_32) & 0xffffffff);
            }
        }
        // Bigger uint can't be represented
        else {
            throw Error("uintBytes > 8");
        }
    }
    setUintBigint(uintBytes, offsetBytes, valueBN) {
        const hIndex = Math.floor(offsetBytes / 4);
        // number has to be masked from an h value
        if (uintBytes < 4) {
            const value = Number(valueBN);
            const bitIndex = (offsetBytes % 4) * 8;
            let h = getNodeH(this, hIndex);
            if (uintBytes === 1) {
                h &= ~(0xff << bitIndex);
                h |= (0xff && value) << bitIndex;
            }
            else {
                h &= ~(0xffff << bitIndex);
                h |= (0xffff && value) << bitIndex;
            }
            setNodeH(this, hIndex, h);
        }
        // number equals the h value
        else if (uintBytes === 4) {
            setNodeH(this, hIndex, Number(valueBN));
        }
        // number spans multiple h values
        else {
            const hEnd = hIndex + Math.ceil(uintBytes / 4);
            for (let i = hIndex; i < hEnd; i++) {
                setNodeH(this, i, Number(valueBN & BigInt(0xffffffff)));
                valueBN = valueBN >> BigInt(32);
            }
        }
    }
    bitwiseOrUint(uintBytes, offsetBytes, value) {
        const hIndex = Math.floor(offsetBytes / 4);
        // number has to be masked from an h value
        if (uintBytes < 4) {
            const bitIndex = (offsetBytes % 4) * 8;
            bitwiseOrNodeH(this, hIndex, value << bitIndex);
        }
        // number equals the h value
        else if (uintBytes === 4) {
            bitwiseOrNodeH(this, hIndex, value);
        }
        // number spans multiple h values
        else {
            const hEnd = hIndex + Math.ceil(uintBytes / 4);
            for (let i = hIndex; i < hEnd; i++) {
                bitwiseOrNodeH(this, i, value & 0xffffffff);
                value >>= 32;
            }
        }
    }
}
exports.LeafNode = LeafNode;
function identity(n) {
    return n;
}
exports.identity = identity;
function compose(inner, outer) {
    return function (n) {
        return outer(inner(n));
    };
}
exports.compose = compose;
function getNodeH(node, hIndex) {
    if (hIndex === 0)
        return node.h0;
    else if (hIndex === 1)
        return node.h1;
    else if (hIndex === 2)
        return node.h2;
    else if (hIndex === 3)
        return node.h3;
    else if (hIndex === 4)
        return node.h4;
    else if (hIndex === 5)
        return node.h5;
    else if (hIndex === 6)
        return node.h6;
    else if (hIndex === 7)
        return node.h7;
    else
        throw Error("hIndex > 7");
}
exports.getNodeH = getNodeH;
function setNodeH(node, hIndex, value) {
    if (hIndex === 0)
        node.h0 = value;
    else if (hIndex === 1)
        node.h1 = value;
    else if (hIndex === 2)
        node.h2 = value;
    else if (hIndex === 3)
        node.h3 = value;
    else if (hIndex === 4)
        node.h4 = value;
    else if (hIndex === 5)
        node.h5 = value;
    else if (hIndex === 6)
        node.h6 = value;
    else if (hIndex === 7)
        node.h7 = value;
    else
        throw Error("hIndex > 7");
}
exports.setNodeH = setNodeH;
function bitwiseOrNodeH(node, hIndex, value) {
    if (hIndex === 0)
        node.h0 |= value;
    else if (hIndex === 1)
        node.h1 |= value;
    else if (hIndex === 2)
        node.h2 |= value;
    else if (hIndex === 3)
        node.h3 |= value;
    else if (hIndex === 4)
        node.h4 |= value;
    else if (hIndex === 5)
        node.h5 |= value;
    else if (hIndex === 6)
        node.h6 |= value;
    else if (hIndex === 7)
        node.h7 |= value;
    else
        throw Error("hIndex > 7");
}
exports.bitwiseOrNodeH = bitwiseOrNodeH;
