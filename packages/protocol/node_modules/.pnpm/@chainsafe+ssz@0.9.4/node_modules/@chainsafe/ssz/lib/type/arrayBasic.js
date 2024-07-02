"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.assertValidArrayLength = exports.value_defaultValueArray = exports.value_equals = exports.value_cloneArray = exports.value_toJsonArray = exports.value_fromJsonArray = exports.tree_deserializeFromBytesArrayBasic = exports.tree_serializeToBytesArrayBasic = exports.value_deserializeFromBytesArrayBasic = exports.value_serializeToBytesArrayBasic = exports.setChunksNode = exports.addLengthNode = exports.getChunksNodeFromRootNode = exports.getLengthFromRootNode = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
// There's a matrix of Array-ish types that require a combination of this functions.
// Regular class extends syntax doesn't work because it can only extend a single class.
//
// Type of array: List, Vector. Changes length property
// Type of element: Basic, Composite. Changes merkelization if packing or not.
// If Composite: Fixed len, Variable len. Changes the serialization requiring offsets.
/**
 * SSZ Lists (variable-length arrays) include the length of the list in the tree
 * This length is always in the same index in the tree
 * ```
 *   1
 *  / \
 * 2   3 // <-here
 * ```
 */
function getLengthFromRootNode(node) {
    // Length is represented as a Uint32 at the start of the chunk:
    // 4 = 4 bytes in Uint32
    // 0 = 0 offset bytes in Node's data
    return node.right.getUint(4, 0);
}
exports.getLengthFromRootNode = getLengthFromRootNode;
function getChunksNodeFromRootNode(node) {
    return node.left;
}
exports.getChunksNodeFromRootNode = getChunksNodeFromRootNode;
function addLengthNode(chunksNode, length) {
    return new persistent_merkle_tree_1.BranchNode(chunksNode, persistent_merkle_tree_1.LeafNode.fromUint32(length));
}
exports.addLengthNode = addLengthNode;
function setChunksNode(rootNode, chunksNode, newLength) {
    const lengthNode = newLength !== undefined
        ? // If newLength is set, create a new node for length
            persistent_merkle_tree_1.LeafNode.fromUint32(newLength)
        : // else re-use existing node
            rootNode.right;
    return new persistent_merkle_tree_1.BranchNode(chunksNode, lengthNode);
}
exports.setChunksNode = setChunksNode;
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
function value_serializeToBytesArrayBasic(elementType, length, output, offset, value) {
    const elSize = elementType.byteLength;
    for (let i = 0; i < length; i++) {
        elementType.value_serializeToBytes(output, offset + i * elSize, value[i]);
    }
    return offset + length * elSize;
}
exports.value_serializeToBytesArrayBasic = value_serializeToBytesArrayBasic;
function value_deserializeFromBytesArrayBasic(elementType, data, start, end, arrayProps) {
    const elSize = elementType.byteLength;
    // Vector + List length validation
    const length = (end - start) / elSize;
    assertValidArrayLength(length, arrayProps, true);
    const values = new Array(length);
    for (let i = 0; i < length; i++) {
        // TODO: If faster, consider skipping size check for uint types
        values[i] = elementType.value_deserializeFromBytes(data, start + i * elSize, start + (i + 1) * elSize);
    }
    return values;
}
exports.value_deserializeFromBytesArrayBasic = value_deserializeFromBytesArrayBasic;
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
function tree_serializeToBytesArrayBasic(elementType, length, depth, output, offset, node) {
    const size = elementType.byteLength * length;
    const chunkCount = Math.ceil(size / 32);
    const nodes = persistent_merkle_tree_1.getNodesAtDepth(node, depth, 0, chunkCount);
    persistent_merkle_tree_1.packedNodeRootsToBytes(output.dataView, offset, size, nodes);
    return offset + size;
}
exports.tree_serializeToBytesArrayBasic = tree_serializeToBytesArrayBasic;
// List of basic elements will pack them in merkelized form
function tree_deserializeFromBytesArrayBasic(elementType, chunkDepth, data, start, end, arrayProps) {
    // Vector + List length validation
    const length = (end - start) / elementType.byteLength;
    assertValidArrayLength(length, arrayProps, true);
    // Abstract converting data to LeafNode to allow for custom data representation, such as the hashObject
    const chunksNode = persistent_merkle_tree_1.packedRootsBytesToNode(chunkDepth, data.dataView, start, end);
    if (arrayProps.isList) {
        return addLengthNode(chunksNode, length);
    }
    else {
        return chunksNode;
    }
}
exports.tree_deserializeFromBytesArrayBasic = tree_deserializeFromBytesArrayBasic;
/**
 * @param length In List length = undefined, Vector length = fixed value
 */
function value_fromJsonArray(elementType, json, arrayProps) {
    if (!Array.isArray(json)) {
        throw Error("JSON is not an array");
    }
    assertValidArrayLength(json.length, arrayProps);
    const value = new Array(json.length);
    for (let i = 0; i < json.length; i++) {
        value[i] = elementType.fromJson(json[i]);
    }
    return value;
}
exports.value_fromJsonArray = value_fromJsonArray;
/**
 * @param length In List length = undefined, Vector length = fixed value
 */
function value_toJsonArray(elementType, value, arrayProps) {
    const length = arrayProps.isList ? value.length : arrayProps.length;
    const json = new Array(length);
    for (let i = 0; i < length; i++) {
        json[i] = elementType.toJson(value[i]);
    }
    return json;
}
exports.value_toJsonArray = value_toJsonArray;
/**
 * Clone recursively an array of basic or composite types
 */
function value_cloneArray(elementType, value) {
    const newValue = new Array(value.length);
    for (let i = 0; i < value.length; i++) {
        newValue[i] = elementType.clone(value[i]);
    }
    return newValue;
}
exports.value_cloneArray = value_cloneArray;
/**
 * Check recursively if a type is structuraly equal. Returns early
 */
function value_equals(elementType, a, b) {
    if (a.length !== b.length) {
        return false;
    }
    for (let i = 0; i < a.length; i++) {
        if (!elementType.equals(a[i], b[i])) {
            return false;
        }
    }
    return true;
}
exports.value_equals = value_equals;
function value_defaultValueArray(elementType, length) {
    const values = new Array(length);
    for (let i = 0; i < length; i++) {
        values[i] = elementType.defaultValue();
    }
    return values;
}
exports.value_defaultValueArray = value_defaultValueArray;
/**
 * @param checkNonDecimalLength Check that length is a multiple of element size.
 * Optional since it's not necessary in getOffsetsArrayComposite() fn.
 */
function assertValidArrayLength(length, arrayProps, checkNonDecimalLength) {
    if (checkNonDecimalLength && length % 1 !== 0) {
        throw Error("size not multiple of element fixedSize");
    }
    // Vector + List length validation
    if (arrayProps.isList) {
        if (length > arrayProps.limit) {
            throw new Error(`Invalid list length ${length} over limit ${arrayProps.limit}`);
        }
    }
    else {
        if (length !== arrayProps.length) {
            throw new Error(`Incorrect vector length ${length} expected ${arrayProps.length}`);
        }
    }
}
exports.assertValidArrayLength = assertValidArrayLength;
//# sourceMappingURL=arrayBasic.js.map