"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.NoneType = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const basic_1 = require("./basic");
/* eslint-disable @typescript-eslint/member-ordering */
/* eslint-disable @typescript-eslint/no-unused-vars */
class NoneType extends basic_1.BasicType {
    constructor() {
        super(...arguments);
        this.typeName = "none";
        this.byteLength = 0;
        this.itemsPerChunk = 32;
        this.fixedSize = 0;
        this.minSize = 0;
        this.maxSize = 0;
    }
    defaultValue() {
        return null;
    }
    // bytes serdes
    value_serializeToBytes(output, offset, value) {
        return offset;
    }
    value_deserializeFromBytes(data, start) {
        return null;
    }
    tree_serializeToBytes(output, offset, node) {
        return offset;
    }
    tree_deserializeFromBytes(data, start, end) {
        return persistent_merkle_tree_1.zeroNode(0);
    }
    // Fast tree opts
    tree_getFromNode(leafNode) {
        return null;
    }
    tree_setToNode(leafNode, value) {
        return;
    }
    tree_getFromPackedNode(leafNode, index) {
        return null;
    }
    tree_setToPackedNode(leafNode, index, value) {
        return;
    }
    // JSON
    fromJson(json) {
        if (json !== null) {
            throw Error("JSON invalid type none must be null");
        }
        return null;
    }
    toJson(value) {
        return null;
    }
}
exports.NoneType = NoneType;
//# sourceMappingURL=none.js.map