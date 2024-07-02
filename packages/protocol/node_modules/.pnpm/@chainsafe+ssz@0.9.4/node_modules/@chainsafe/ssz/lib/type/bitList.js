"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BitListType = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const arrayBasic_1 = require("./arrayBasic");
const bitArray_1 = require("../value/bitArray");
const bitArray_2 = require("./bitArray");
/**
 * BitList: ordered variable-length collection of boolean values, limited to N bits
 * - Notation `Bitlist[N]`
 * - Value: `BitArray`, @see BitArray for a justification of its memory efficiency and performance
 * - View: `BitArrayTreeView`
 * - ViewDU: `BitArrayTreeViewDU`
 */
class BitListType extends bitArray_2.BitArrayType {
    constructor(limitBits, opts) {
        super();
        this.limitBits = limitBits;
        this.fixedSize = null;
        this.minSize = 1; // +1 for the extra padding bit
        this.isList = true;
        if (limitBits === 0)
            throw Error("List limit must be > 0");
        this.typeName = opts?.typeName ?? `BitList[${limitBits}]`;
        // TODO Check that itemsPerChunk is an integer
        this.maxChunkCount = Math.ceil(this.limitBits / 8 / 32);
        this.chunkDepth = merkleize_1.maxChunksToDepth(this.maxChunkCount);
        // Depth includes the extra level for the length node
        this.depth = 1 + this.chunkDepth;
        this.maxSize = Math.ceil(limitBits / 8) + 1; // +1 for the extra padding bit
    }
    static named(limitBits, opts) {
        return new (named_1.namedClass(BitListType, opts.typeName))(limitBits, opts);
    }
    defaultValue() {
        return bitArray_1.BitArray.fromBitLen(0);
    }
    // Views: inherited from BitArrayType
    // Serialization + deserialization
    value_serializedSize(value) {
        return bitLenToSerializedLength(value.bitLen);
    }
    value_serializeToBytes(output, offset, value) {
        output.uint8Array.set(value.uint8Array, offset);
        return applyPaddingBit(output.uint8Array, offset, value.bitLen);
    }
    value_deserializeFromBytes(data, start, end) {
        const { uint8Array, bitLen } = this.deserializeUint8ArrayBitListFromBytes(data.uint8Array, start, end);
        return new bitArray_1.BitArray(uint8Array, bitLen);
    }
    tree_serializedSize(node) {
        return bitLenToSerializedLength(arrayBasic_1.getLengthFromRootNode(node));
    }
    tree_serializeToBytes(output, offset, node) {
        const chunksNode = arrayBasic_1.getChunksNodeFromRootNode(node);
        const bitLen = arrayBasic_1.getLengthFromRootNode(node);
        const byteLen = Math.ceil(bitLen / 8);
        const chunkLen = Math.ceil(byteLen / 32);
        const nodes = persistent_merkle_tree_1.getNodesAtDepth(chunksNode, this.chunkDepth, 0, chunkLen);
        persistent_merkle_tree_1.packedNodeRootsToBytes(output.dataView, offset, byteLen, nodes);
        return applyPaddingBit(output.uint8Array, offset, bitLen);
    }
    tree_deserializeFromBytes(data, start, end) {
        const { uint8Array, bitLen } = this.deserializeUint8ArrayBitListFromBytes(data.uint8Array, start, end);
        const dataView = new DataView(uint8Array.buffer, uint8Array.byteOffset, uint8Array.byteLength);
        const chunksNode = persistent_merkle_tree_1.packedRootsBytesToNode(this.chunkDepth, dataView, 0, uint8Array.length);
        return arrayBasic_1.addLengthNode(chunksNode, bitLen);
    }
    tree_getByteLen(node) {
        if (!node)
            throw new Error("BitListType requires a node to get leaves");
        return Math.ceil(arrayBasic_1.getLengthFromRootNode(node) / 8);
    }
    // Merkleization: inherited from BitArrayType
    hashTreeRoot(value) {
        return merkleize_1.mixInLength(super.hashTreeRoot(value), value.bitLen);
    }
    // Proofs: inherited from BitArrayType
    // JSON: inherited from BitArrayType
    // Deserializer helpers
    deserializeUint8ArrayBitListFromBytes(data, start, end) {
        const { uint8Array, bitLen } = deserializeUint8ArrayBitListFromBytes(data, start, end);
        if (bitLen > this.limitBits) {
            throw Error(`bitLen over limit ${bitLen} > ${this.limitBits}`);
        }
        return { uint8Array, bitLen };
    }
}
exports.BitListType = BitListType;
function deserializeUint8ArrayBitListFromBytes(data, start, end) {
    if (end > data.length) {
        throw Error(`BitList attempting to read byte ${end} of data length ${data.length}`);
    }
    const lastByte = data[end - 1];
    const size = end - start;
    if (lastByte === 0) {
        throw new Error("Invalid deserialized bitlist, padding bit required");
    }
    if (lastByte === 1) {
        // Buffer.prototype.slice does not copy memory, Enforce Uint8Array usage https://github.com/nodejs/node/issues/28087
        const uint8Array = Uint8Array.prototype.slice.call(data, start, end - 1);
        const bitLen = (size - 1) * 8;
        return { uint8Array, bitLen };
    }
    // the last byte is > 1, so a padding bit will exist in the last byte and need to be removed
    // Buffer.prototype.slice does not copy memory, Enforce Uint8Array usage https://github.com/nodejs/node/issues/28087
    const uint8Array = Uint8Array.prototype.slice.call(data, start, end);
    // mask lastChunkByte
    const lastByteBitLength = lastByte.toString(2).length - 1;
    const bitLen = (size - 1) * 8 + lastByteBitLength;
    const mask = 0xff >> (8 - lastByteBitLength);
    uint8Array[size - 1] &= mask;
    return { uint8Array, bitLen };
}
function bitLenToSerializedLength(bitLen) {
    const bytes = Math.ceil(bitLen / 8);
    // +1 for the extra padding bit
    return bitLen % 8 === 0 ? bytes + 1 : bytes;
}
/**
 * Apply padding bit to a serialized BitList already written to `output` at `offset`
 * @returns New offset after (maybe) writting a padding bit.
 */
function applyPaddingBit(output, offset, bitLen) {
    const byteLen = Math.ceil(bitLen / 8);
    const newOffset = offset + byteLen;
    if (bitLen % 8 === 0) {
        output[newOffset] = 1;
        return newOffset + 1;
    }
    else {
        output[newOffset - 1] |= 1 << bitLen % 8;
        return newOffset;
    }
}
//# sourceMappingURL=bitList.js.map