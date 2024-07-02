import { CompositeType } from "../type/composite";
import { TreeView } from "../view/abstract";
/**
 * A Deferred Update Tree View (`ViewDU`) is a wrapper around a type and
 * a SSZ Node that contains:
 * - data merkleized
 * - some arbitrary caches to speed up data manipulation required by the type
 *
 * **ViewDU**
 * - Best for complex usage where performance is important
 * - Defers changes to when commit is called
 * - Does NOT have a reference to the parent ViewDU
 * - Has caches for fast get / set ops
 */
export declare abstract class TreeViewDU<T extends CompositeType<unknown, unknown, unknown>> extends TreeView<T> {
    /**
     * Applies any deferred updates that may be pending in this ViewDU instance and updates its internal `Node`.
     */
    abstract commit(): void;
    /**
     * Returns arbitrary data that is useful for this ViewDU instance to optimize data manipulation. This caches MUST
     * not include non-commited data. `this.cache` can be called at any time, both before and after calling `commit()`.
     */
    abstract readonly cache: unknown;
    /**
     * MUST drop any reference to mutable cache data. After `clearCache()`, if the dropped caches are mutated, no changes
     * should apply to this instance both before and after calling `commit()`.
     */
    protected abstract clearCache(): void;
    /**
     * Merkleize view and compute its hashTreeRoot.
     * Commits any pending changes before computing the root.
     *
     * See spec for definition of hashTreeRoot:
     * https://github.com/ethereum/consensus-specs/blob/dev/ssz/simple-serialize.md#merkleization
     */
    hashTreeRoot(): Uint8Array;
    /**
     * Serialize view to binary data.
     * Commits any pending changes before computing the root.
     */
    serialize(): Uint8Array;
    /**
     * Return a new ViewDU instance referencing the same internal `Node`.
     *
     * By default it will transfer the cache of this ViewDU to the new cloned instance. Set `dontTransferCache` to true
     * to NOT transfer the cache to the cloned instance.
     */
    clone(dontTransferCache?: boolean): this;
}
//# sourceMappingURL=abstract.d.ts.map