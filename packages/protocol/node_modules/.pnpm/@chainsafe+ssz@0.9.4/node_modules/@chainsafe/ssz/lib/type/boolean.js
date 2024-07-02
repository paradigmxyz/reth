"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BooleanType = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const named_1 = require("../util/named");
const basic_1 = require("./basic");
/**
 * Boolean: True or False
 * - Notation: `boolean`
 */
class BooleanType extends basic_1.BasicType {
    constructor(opts) {
        super();
        this.byteLength = 1;
        this.itemsPerChunk = 32;
        this.fixedSize = 1;
        this.minSize = 1;
        this.maxSize = 1;
        this.typeName = opts?.typeName ?? "boolean";
    }
    static named(opts) {
        return new (named_1.namedClass(BooleanType, opts.typeName))(opts);
    }
    defaultValue() {
        return false;
    }
    // Serialization + deserialization
    value_serializeToBytes(output, offset, value) {
        output.uint8Array[offset] = value ? 1 : 0;
        return offset + 1;
    }
    value_deserializeFromBytes(data, start, end) {
        this.assertValidSize(end - start);
        switch (data.uint8Array[start]) {
            case 1:
                return true;
            case 0:
                return false;
            default:
                throw new Error(`Boolean: invalid value: ${data.uint8Array[start]}`);
        }
    }
    tree_serializeToBytes(output, offset, node) {
        // TODO: Assumes LeafNode has 4 byte uints are primary unit
        output.uint8Array[offset] = node.getUint(4, 0);
        return offset + 1;
    }
    tree_deserializeFromBytes(data, start, end) {
        this.assertValidSize(end - start);
        const value = data.uint8Array[start];
        if (value > 1) {
            throw Error(`Boolean: invalid value ${value}`);
        }
        return persistent_merkle_tree_1.LeafNode.fromUint32(value);
    }
    // Fast tree opts
    tree_getFromNode(leafNode) {
        return leafNode.getUint(4, 0) === 1;
    }
    tree_setToNode(leafNode, value) {
        leafNode.setUint(4, 0, value ? 1 : 0);
    }
    tree_getFromPackedNode(leafNode, index) {
        const offsetBytes = index % this.itemsPerChunk;
        return leafNode.getUint(1, offsetBytes) !== 0;
    }
    tree_setToPackedNode(leafNode, index, value) {
        const offsetBytes = index % this.itemsPerChunk;
        leafNode.setUint(1, offsetBytes, value ? 1 : 0);
    }
    // JSON
    fromJson(json) {
        if (typeof json !== "boolean") {
            throw Error(`JSON invalid type ${typeof json} expected boolean`);
        }
        return json;
    }
    toJson(value) {
        return value;
    }
}
exports.BooleanType = BooleanType;
//# sourceMappingURL=boolean.js.map