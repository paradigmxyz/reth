"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.value_getRootsArrayComposite = exports.tree_deserializeFromBytesArrayComposite = exports.tree_serializeToBytesArrayComposite = exports.tree_serializedSizeArrayComposite = exports.value_deserializeFromBytesArrayComposite = exports.value_serializeToBytesArrayComposite = exports.value_serializedSizeArrayComposite = exports.maxSizeArrayComposite = exports.minSizeArrayComposite = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const arrayBasic_1 = require("./arrayBasic");
function minSizeArrayComposite(elementType, minCount) {
    // Variable Length
    if (elementType.fixedSize === null) {
        return minCount * (4 + elementType.minSize);
    }
    // Fixed length
    else {
        return minCount * elementType.minSize;
    }
}
exports.minSizeArrayComposite = minSizeArrayComposite;
function maxSizeArrayComposite(elementType, maxCount) {
    // Variable Length
    if (elementType.fixedSize === null) {
        return maxCount * (4 + elementType.maxSize);
    }
    // Fixed length
    else {
        return maxCount * elementType.maxSize;
    }
}
exports.maxSizeArrayComposite = maxSizeArrayComposite;
function value_serializedSizeArrayComposite(elementType, length, value) {
    // Variable Length
    if (elementType.fixedSize === null) {
        let size = 0;
        for (let i = 0; i < length; i++) {
            size += 4 + elementType.value_serializedSize(value[i]);
        }
        return size;
    }
    // Fixed length
    else {
        return length * elementType.fixedSize;
    }
}
exports.value_serializedSizeArrayComposite = value_serializedSizeArrayComposite;
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
function value_serializeToBytesArrayComposite(elementType, length, output, offset, value) {
    // Variable length
    if (elementType.fixedSize === null) {
        let variableIndex = offset + length * 4;
        for (let i = 0; i < length; i++) {
            // write offset
            output.dataView.setUint32(offset + i * 4, variableIndex - offset, true);
            // write serialized element to variable section
            variableIndex = elementType.value_serializeToBytes(output, variableIndex, value[i]);
        }
        return variableIndex;
    }
    // Fixed length
    else {
        for (let i = 0; i < length; i++) {
            elementType.value_serializeToBytes(output, offset + i * elementType.fixedSize, value[i]);
        }
        return offset + length * elementType.fixedSize;
    }
}
exports.value_serializeToBytesArrayComposite = value_serializeToBytesArrayComposite;
function value_deserializeFromBytesArrayComposite(elementType, data, start, end, arrayProps) {
    const offsets = readOffsetsArrayComposite(elementType.fixedSize, data.dataView, start, end, arrayProps);
    const length = offsets.length; // Capture length before pushing end offset
    const values = new Array(length);
    // offests include the last element end
    for (let i = 0; i < length; i++) {
        // The offsets are relative to the start
        const startEl = start + offsets[i];
        const endEl = i === length - 1 ? end : start + offsets[i + 1];
        values[i] = elementType.value_deserializeFromBytes(data, startEl, endEl);
    }
    return values;
}
exports.value_deserializeFromBytesArrayComposite = value_deserializeFromBytesArrayComposite;
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
function tree_serializedSizeArrayComposite(elementType, length, depth, node) {
    // Variable Length
    if (elementType.fixedSize === null) {
        const nodes = persistent_merkle_tree_1.getNodesAtDepth(node, depth, 0, length);
        let size = 0;
        for (let i = 0; i < nodes.length; i++) {
            size += 4 + elementType.tree_serializedSize(nodes[i]);
        }
        return size;
    }
    // Fixed length
    else {
        return length * elementType.fixedSize;
    }
}
exports.tree_serializedSizeArrayComposite = tree_serializedSizeArrayComposite;
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
function tree_serializeToBytesArrayComposite(elementType, length, depth, node, output, offset) {
    const nodes = persistent_merkle_tree_1.getNodesAtDepth(node, depth, 0, length);
    // Variable Length
    // Indices contain offsets, which are indices deeper in the byte array
    if (elementType.fixedSize === null) {
        let variableIndex = offset + length * 4;
        const { dataView } = output;
        for (let i = 0; i < nodes.length; i++) {
            // write offset
            dataView.setUint32(offset + i * 4, variableIndex - offset, true);
            // write serialized element to variable section
            variableIndex = elementType.tree_serializeToBytes(output, variableIndex, nodes[i]);
        }
        return variableIndex;
    }
    // Fixed length
    else {
        for (let i = 0; i < nodes.length; i++) {
            offset = elementType.tree_serializeToBytes(output, offset, nodes[i]);
        }
        return offset;
    }
}
exports.tree_serializeToBytesArrayComposite = tree_serializeToBytesArrayComposite;
function tree_deserializeFromBytesArrayComposite(elementType, chunkDepth, data, start, end, arrayProps) {
    const offsets = readOffsetsArrayComposite(elementType.fixedSize, data.dataView, start, end, arrayProps);
    const length = offsets.length; // Capture length before pushing end offset
    const nodes = new Array(length);
    // offests include the last element end
    for (let i = 0; i < length; i++) {
        // The offsets are relative to the start
        const startEl = start + offsets[i];
        const endEl = i === length - 1 ? end : start + offsets[i + 1];
        nodes[i] = elementType.tree_deserializeFromBytes(data, startEl, endEl);
    }
    // Abstract converting data to LeafNode to allow for custom data representation, such as the hashObject
    const chunksNode = persistent_merkle_tree_1.subtreeFillToContents(nodes, chunkDepth);
    // TODO: Add LeafNode.fromUint()
    if (arrayProps.isList) {
        return arrayBasic_1.addLengthNode(chunksNode, length);
    }
    else {
        return chunksNode;
    }
}
exports.tree_deserializeFromBytesArrayComposite = tree_deserializeFromBytesArrayComposite;
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
function value_getRootsArrayComposite(elementType, length, value) {
    const roots = new Array(length);
    for (let i = 0; i < length; i++) {
        roots[i] = elementType.hashTreeRoot(value[i]);
    }
    return roots;
}
exports.value_getRootsArrayComposite = value_getRootsArrayComposite;
function readOffsetsArrayComposite(elementFixedSize, data, start, end, arrayProps) {
    const size = end - start;
    let offsets;
    // Variable Length
    // Indices contain offsets, which are indices deeper in the byte array
    if (elementFixedSize === null) {
        offsets = readVariableOffsetsArrayComposite(data, start, size);
    }
    // Fixed length
    else {
        // There's no valid CompositeType with fixed size 0, it's un-rechable code. But prevents diving by zero
        /* istanbul ignore if */
        if (elementFixedSize === 0) {
            throw Error("element fixed length is 0");
        }
        if (size % elementFixedSize !== 0) {
            throw Error(`size ${size} is not multiple of element fixedSize ${elementFixedSize}`);
        }
        const length = size / elementFixedSize;
        offsets = new Uint32Array(length);
        for (let i = 0; i < length; i++) {
            offsets[i] = i * elementFixedSize;
        }
    }
    // Vector + List length validation
    arrayBasic_1.assertValidArrayLength(offsets.length, arrayProps);
    return offsets;
}
/**
 * Reads the values of contiguous variable offsets. Provided buffer includes offsets that point to position
 * within `size`. This function also validates that all offsets are in range.
 */
function readVariableOffsetsArrayComposite(dataView, start, size) {
    if (size === 0) {
        return new Uint32Array(0);
    }
    // all elements are variable-sized
    // indices contain offsets, which are indices deeper in the byte array
    // The serialized data will start with offsets of all the serialized objects (BYTES_PER_LENGTH_OFFSET bytes each)
    const firstOffset = dataView.getUint32(start, true);
    // Using the first offset, we can compute the length of the list (divide by BYTES_PER_LENGTH_OFFSET), as it gives
    // us the total number of bytes in the offset data
    const offsetDataLength = firstOffset;
    if (firstOffset === 0) {
        throw Error("First offset must be > 0");
    }
    if (offsetDataLength % 4 !== 0) {
        throw Error("Offset data length not multiple of 4");
    }
    const offsetCount = offsetDataLength / 4;
    const offsets = new Uint32Array(offsetCount);
    offsets[0] = firstOffset;
    // ArrayComposite has a contiguous section of offsets, then the data
    //
    //    [offset 1] [offset 2] [data 1 ..........] [data 2 ..]
    // 0x 08000000   0e000000   010002000300        01000200
    //
    // Ensure that:
    // - Offsets point to regions of > 0 bytes, i.e. are increasing
    // - Offsets don't point to bytes outside of the array's size
    //
    // In the example above the first offset is 8, so 8 / 4 = 2 offsets.
    // Then, read the rest of offsets to get offsets = [8, 14]
    for (let offsetIdx = 1; offsetIdx < offsetCount; offsetIdx++) {
        const offset = dataView.getUint32(start + offsetIdx * 4, true);
        offsets[offsetIdx] = offset;
        // Offsets must point to data within the Array bytes section
        if (offset > size) {
            throw new Error(`Offset out of bounds ${offset} > ${size}`);
        }
        if (offset < offsets[offsetIdx - 1]) {
            throw new Error(`Offsets must be increasing ${offset} < ${offsets[offsetIdx - 1]}`);
        }
    }
    return offsets;
}
//# sourceMappingURL=arrayComposite.js.map