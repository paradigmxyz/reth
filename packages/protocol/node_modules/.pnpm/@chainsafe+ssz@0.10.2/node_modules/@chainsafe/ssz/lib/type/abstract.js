"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.Type = void 0;
/**
 * An SSZ type provides the following operations:
 * - Serialization from/to bytes to either a value or a tree
 * - Merkelization to compute the hashTreeRoot of both a value and a tree
 * - Proof creation from trees
 * - Create a View and a ViewDU instance from a tree
 * - Manipulate views
 */
class Type {
    /** INTERNAL METHOD: Merkleize value to tree */
    value_toTree(value) {
        // TODO: Un-performant path but useful for prototyping. Overwrite in Type if performance is important
        const uint8Array = new Uint8Array(this.value_serializedSize(value));
        const dataView = new DataView(uint8Array.buffer, uint8Array.byteOffset, uint8Array.byteLength);
        this.value_serializeToBytes({ uint8Array, dataView }, 0, value);
        return this.tree_deserializeFromBytes({ uint8Array, dataView }, 0, uint8Array.length);
    }
    /** INTERNAL METHOD: Un-merkleize tree to value */
    tree_toValue(node) {
        // TODO: Un-performant path but useful for prototyping. Overwrite in Type if performance is important
        const uint8Array = new Uint8Array(this.tree_serializedSize(node));
        const dataView = new DataView(uint8Array.buffer, uint8Array.byteOffset, uint8Array.byteLength);
        this.tree_serializeToBytes({ uint8Array, dataView }, 0, node);
        return this.value_deserializeFromBytes({ uint8Array, dataView }, 0, uint8Array.length);
    }
    /** Serialize a value to binary data */
    serialize(value) {
        const uint8Array = new Uint8Array(this.value_serializedSize(value));
        const dataView = new DataView(uint8Array.buffer, uint8Array.byteOffset, uint8Array.byteLength);
        this.value_serializeToBytes({ uint8Array, dataView }, 0, value);
        return uint8Array;
    }
    /** Deserialize binary data to value */
    deserialize(uint8Array) {
        // Buffer.prototype.slice does not copy memory, force use Uint8Array.prototype.slice https://github.com/nodejs/node/issues/28087
        // - Uint8Array.prototype.slice: Copy memory, safe to mutate
        // - Buffer.prototype.slice: Does NOT copy memory, mutation affects both views
        // We could ensure that all Buffer instances are converted to Uint8Array before calling value_deserializeFromBytes
        // However doing that in a browser friendly way is not easy. Downstream code uses `Uint8Array.prototype.slice.call`
        // to ensure Buffer.prototype.slice is never used. Unit tests also test non-mutability.
        const dataView = new DataView(uint8Array.buffer, uint8Array.byteOffset, uint8Array.byteLength);
        return this.value_deserializeFromBytes({ uint8Array, dataView }, 0, uint8Array.length);
    }
}
exports.Type = Type;
//# sourceMappingURL=abstract.js.map