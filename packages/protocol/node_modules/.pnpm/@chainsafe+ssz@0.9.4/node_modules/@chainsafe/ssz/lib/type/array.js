"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ArrayType = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const composite_1 = require("./composite");
const arrayBasic_1 = require("./arrayBasic");
/* eslint-disable @typescript-eslint/member-ordering */
/**
 * Array: ordered homogeneous collection
 */
class ArrayType extends composite_1.CompositeType {
    constructor(elementType) {
        super();
        this.elementType = elementType;
    }
    defaultValue() {
        return arrayBasic_1.value_defaultValueArray(this.elementType, this.defaultLen);
    }
    // Proofs
    getPropertyType() {
        return this.elementType;
    }
    getPropertyGindex(prop) {
        if (typeof prop !== "number") {
            throw Error(`Invalid array index: ${prop}`);
        }
        const chunkIdx = Math.floor(prop / this.itemsPerChunk);
        return persistent_merkle_tree_1.toGindex(this.depth, BigInt(chunkIdx));
    }
    getIndexProperty(index) {
        return index;
    }
    tree_getLeafGindices(rootGindex, rootNode) {
        let length;
        if (this.isList) {
            if (!rootNode) {
                throw new Error("List type requires tree argument to get leaves");
            }
            length = this.tree_getLength(rootNode);
        }
        else {
            // Vectors don't need a rootNode to return length
            length = this.tree_getLength(null);
        }
        const gindices = [];
        if (composite_1.isCompositeType(this.elementType)) {
            // Underlying elements exist one per chunk
            // Iterate through chunk gindices, recursively fetching leaf gindices from each chunk
            const startIndex = persistent_merkle_tree_1.toGindex(this.depth, BigInt(0));
            const endGindex = startIndex + BigInt(length);
            const extendedStartIndex = persistent_merkle_tree_1.concatGindices([rootGindex, startIndex]);
            if (this.elementType.fixedSize === null) {
                if (!rootNode) {
                    /* istanbul ignore next - unreachable code */
                    throw new Error("Array of variable size requires tree argument to get leaves");
                }
                // variable-length elements must pass the underlying subtrees to determine the length
                for (let gindex = startIndex, extendedGindex = extendedStartIndex; gindex < endGindex; gindex++, extendedGindex++) {
                    gindices.push(...this.elementType.tree_getLeafGindices(extendedGindex, persistent_merkle_tree_1.getNode(rootNode, gindex)));
                }
            }
            else {
                for (let i = 0, extendedGindex = extendedStartIndex; i < length; i++, extendedGindex++) {
                    gindices.push(...this.elementType.tree_getLeafGindices(extendedGindex));
                }
            }
        }
        // Basic
        else {
            const chunkCount = Math.ceil(length / this.itemsPerChunk);
            const startIndex = persistent_merkle_tree_1.concatGindices([rootGindex, persistent_merkle_tree_1.toGindex(this.depth, BigInt(0))]);
            const endGindex = startIndex + BigInt(chunkCount);
            for (let gindex = startIndex; gindex < endGindex; gindex++) {
                gindices.push(gindex);
            }
        }
        // include the length chunk
        if (this.isList) {
            gindices.push(persistent_merkle_tree_1.concatGindices([rootGindex, composite_1.LENGTH_GINDEX]));
        }
        return gindices;
    }
    // JSON
    fromJson(json) {
        // TODO: Do a better typesafe approach, all final classes of ArrayType implement ArrayProps
        // There are multiple tests that cover this path for all clases
        return arrayBasic_1.value_fromJsonArray(this.elementType, json, this);
    }
    toJson(value) {
        return arrayBasic_1.value_toJsonArray(this.elementType, value, this);
    }
    clone(value) {
        return arrayBasic_1.value_cloneArray(this.elementType, value);
    }
    equals(a, b) {
        return arrayBasic_1.value_equals(this.elementType, a, b);
    }
}
exports.ArrayType = ArrayType;
//# sourceMappingURL=array.js.map