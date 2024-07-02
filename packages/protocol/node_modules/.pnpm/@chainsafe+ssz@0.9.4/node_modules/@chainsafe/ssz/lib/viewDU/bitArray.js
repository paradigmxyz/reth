"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BitArrayTreeViewDU = void 0;
const abstract_1 = require("./abstract");
/**
 * Thin wrapper around BitArray to upstream changes after `this.commit()`
 */
class BitArrayTreeViewDU extends abstract_1.TreeViewDU {
    constructor(type, _rootNode) {
        super();
        this.type = type;
        this._rootNode = _rootNode;
        /** Cached BitArray instance computed only on demand */
        this._bitArray = null;
    }
    get node() {
        return this._rootNode;
    }
    get cache() {
        return;
    }
    commit() {
        if (this._bitArray !== null) {
            this._rootNode = this.type.value_toTree(this._bitArray);
        }
    }
    // Wrapped API from BitArray
    /** @see BitArray.uint8Array */
    get uint8Array() {
        return this.bitArray.uint8Array;
    }
    /** @see BitArray.bitLen */
    get bitLen() {
        return this.bitArray.bitLen;
    }
    /** @see BitArray.get */
    get(bitIndex) {
        return this.bitArray.get(bitIndex);
    }
    /** @see BitArray.set */
    set(bitIndex, bit) {
        this.bitArray.set(bitIndex, bit);
    }
    /** @see BitArray.mergeOrWith */
    mergeOrWith(bitArray2) {
        this.bitArray.mergeOrWith(bitArray2);
    }
    /** @see BitArray.intersectValues */
    intersectValues(values) {
        return this.bitArray.intersectValues(values);
    }
    /** @see BitArray.getTrueBitIndexes */
    getTrueBitIndexes() {
        return this.bitArray.getTrueBitIndexes();
    }
    /** @see BitArray.getSingleTrueBit */
    getSingleTrueBit() {
        return this.bitArray.getSingleTrueBit();
    }
    /** @see BitArray.toBoolArray */
    toBoolArray() {
        return this.bitArray.toBoolArray();
    }
    /** Lazily computed bitArray instance */
    get bitArray() {
        if (this._bitArray === null) {
            this._bitArray = this.type.tree_toValue(this._rootNode);
        }
        return this._bitArray;
    }
    clearCache() {
        this._bitArray = null;
    }
}
exports.BitArrayTreeViewDU = BitArrayTreeViewDU;
//# sourceMappingURL=bitArray.js.map