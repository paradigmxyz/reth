import { Node, Proof } from "@chainsafe/persistent-merkle-tree";
import { ValueOf, JsonPath } from "../type/abstract";
import { CompositeType } from "../type/composite";
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
export declare abstract class TreeView<T extends CompositeType<unknown, unknown, unknown>> {
    /** Merkle tree root node */
    abstract readonly node: Node;
    /** SSZ type associated with this Tree View */
    abstract readonly type: T;
    /** Serialize view to binary data */
    serialize(): Uint8Array;
    /**
     * Merkleize view and compute its hashTreeRoot.
     *
     * See spec for definition of hashTreeRoot:
     * https://github.com/ethereum/consensus-specs/blob/dev/ssz/simple-serialize.md#merkleization
     */
    hashTreeRoot(): Uint8Array;
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
    createProof(paths: JsonPath[]): Proof;
    /**
     * Transform the view into a value, from the current node instance.
     * For ViewDU returns the value of the committed data, so call .commit() before if there are pending changes.
     */
    toValue(): ValueOf<T>;
    /** Return a new Tree View instance referencing the same internal `Node`. Drops its existing `Tree` hook if any */
    clone(): this;
}
//# sourceMappingURL=abstract.d.ts.map