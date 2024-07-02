"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BitArrayTreeView = void 0;
const abstract_1 = require("./abstract");
/**
 * Thin wrapper around BitArray to upstream changes to `tree` on every `this.set()`
 */
class BitArrayTreeView extends abstract_1.TreeView {
    constructor(type, tree) {
        super();
        this.type = type;
        this.tree = tree;
        this.bitArray = type.tree_toValue(tree.rootNode);
    }
    get node() {
        return this.tree.rootNode;
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
        // Upstream changes
        this.tree.rootNode = this.type.value_toTree(this.bitArray);
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
}
exports.BitArrayTreeView = BitArrayTreeView;
//# sourceMappingURL=bitArray.js.map