"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.TreeViewDU = void 0;
const abstract_1 = require("../view/abstract");
/* eslint-disable @typescript-eslint/member-ordering  */
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
class TreeViewDU extends abstract_1.TreeView {
    /**
     * Merkleize view and compute its hashTreeRoot.
     * Commits any pending changes before computing the root.
     *
     * See spec for definition of hashTreeRoot:
     * https://github.com/ethereum/consensus-specs/blob/dev/ssz/simple-serialize.md#merkleization
     */
    hashTreeRoot() {
        this.commit();
        return super.hashTreeRoot();
    }
    /**
     * Serialize view to binary data.
     * Commits any pending changes before computing the root.
     */
    serialize() {
        this.commit();
        return super.serialize();
    }
    /**
     * Return a new ViewDU instance referencing the same internal `Node`.
     *
     * By default it will transfer the cache of this ViewDU to the new cloned instance. Set `dontTransferCache` to true
     * to NOT transfer the cache to the cloned instance.
     */
    clone(dontTransferCache) {
        if (dontTransferCache) {
            return this.type.getViewDU(this.node);
        }
        else {
            const cache = this.cache;
            this.clearCache();
            return this.type.getViewDU(this.node, cache);
        }
    }
}
exports.TreeViewDU = TreeViewDU;
//# sourceMappingURL=abstract.js.map