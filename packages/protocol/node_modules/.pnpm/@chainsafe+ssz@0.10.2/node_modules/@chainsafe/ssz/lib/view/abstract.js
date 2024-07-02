"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.TreeView = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
/**
 * A Tree View is a wrapper around a type and an SSZ Tree that contains:
 * - data merkleized
 * - a hook to its parent Tree to propagate changes upwards
 *
 * **View**
 * - Best for simple usage where performance is NOT important
 * - Applies changes immediately
 * - Has reference to parent tree
 * - Does NOT have caches for fast get / set ops
 */
class TreeView {
    /** Serialize view to binary data */
    serialize() {
        const output = new Uint8Array(this.type.tree_serializedSize(this.node));
        const dataView = new DataView(output.buffer, output.byteOffset, output.byteLength);
        this.type.tree_serializeToBytes({ uint8Array: output, dataView }, 0, this.node);
        return output;
    }
    /**
     * Merkleize view and compute its hashTreeRoot.
     *
     * See spec for definition of hashTreeRoot:
     * https://github.com/ethereum/consensus-specs/blob/dev/ssz/simple-serialize.md#merkleization
     */
    hashTreeRoot() {
        return this.node.root;
    }
    /**
     * Create a Merkle multiproof on this view's data.
     * A `path` is an array of 'JSON' paths into the data
     * @example
     * ```ts
     * state.createProof([
     *   ["validators", 1234, "slashed"],
     *   ["genesisTime"]
     * ])
     * ```
     *
     * See spec for definition of merkle multiproofs:
     * https://github.com/ethereum/consensus-specs/blob/dev/ssz/merkle-proofs.md#merkle-multiproofs
     */
    createProof(paths) {
        return this.type.tree_createProof(this.node, paths);
    }
    /**
     * Transform the view into a value, from the current node instance.
     * For ViewDU returns the value of the committed data, so call .commit() before if there are pending changes.
     */
    toValue() {
        return this.type.tree_toValue(this.node);
    }
    /** Return a new Tree View instance referencing the same internal `Node`. Drops its existing `Tree` hook if any */
    clone() {
        return this.type.getView(new persistent_merkle_tree_1.Tree(this.node));
    }
}
exports.TreeView = TreeView;
//# sourceMappingURL=abstract.js.map