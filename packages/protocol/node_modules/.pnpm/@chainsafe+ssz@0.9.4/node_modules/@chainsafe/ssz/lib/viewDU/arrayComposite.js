"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ArrayCompositeTreeViewDU = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const abstract_1 = require("./abstract");
class ArrayCompositeTreeViewDU extends abstract_1.TreeViewDU {
    constructor(type, _rootNode, cache) {
        super();
        this.type = type;
        this._rootNode = _rootNode;
        this.viewsChanged = new Map();
        // TODO: Consider these properties are not accessible in the cache object persisted in the parent's cache.
        // nodes, caches, _length, and nodesPopulated are mutated. Consider having them in a _cache object such that
        // mutations affect the cache already found in the parent object
        this.dirtyLength = false;
        if (cache) {
            this.nodes = cache.nodes;
            this.caches = cache.caches;
            this._length = cache.length;
            this.nodesPopulated = cache.nodesPopulated;
        }
        else {
            this.nodes = [];
            this.caches = [];
            this._length = this.type.tree_getLength(_rootNode);
            // If there are exactly 0 nodes, nodesPopulated = true because 0 / 0 are in the nodes array
            this.nodesPopulated = this._length === 0;
        }
    }
    /**
     * Number of elements in the array. Equal to un-commited length of the array
     */
    get length() {
        return this._length;
    }
    get node() {
        return this._rootNode;
    }
    get cache() {
        return {
            nodes: this.nodes,
            caches: this.caches,
            length: this._length,
            nodesPopulated: this.nodesPopulated,
        };
    }
    /**
     * Get element at `index`. Returns a view of the Composite element type.
     *
     * NOTE: Assumes that any view created here will change and will call .commit() on it.
     * .get() should be used only for cases when something may mutate. To get all items without
     * triggering a .commit() in all them use .getAllReadOnly().
     */
    get(index) {
        const viewChanged = this.viewsChanged.get(index);
        if (viewChanged) {
            return viewChanged;
        }
        let node = this.nodes[index];
        if (node === undefined) {
            node = persistent_merkle_tree_1.getNodeAtDepth(this._rootNode, this.type.depth, index);
            this.nodes[index] = node;
        }
        // Keep a reference to the new view to call .commit on it latter, only if mutable
        const view = this.type.elementType.getViewDU(node, this.caches[index]);
        if (this.type.elementType.isViewMutable) {
            this.viewsChanged.set(index, view);
        }
        // No need to persist the child's view cache since a second get returns this view instance.
        // The cache is only persisted on commit where the viewsChanged map is dropped.
        return view;
    }
    /**
     * Get element at `index`. Returns a view of the Composite element type.
     * DOES NOT PROPAGATE CHANGES: use only for reads and to skip parent references.
     */
    getReadonly(index) {
        const viewChanged = this.viewsChanged.get(index);
        if (viewChanged) {
            return viewChanged;
        }
        let node = this.nodes[index];
        if (node === undefined) {
            node = persistent_merkle_tree_1.getNodeAtDepth(this._rootNode, this.type.depth, index);
            this.nodes[index] = node;
        }
        return this.type.elementType.getViewDU(node, this.caches[index]);
    }
    // Did not implemented
    // `getReadonlyValue(index: number): ValueOf<ElementType>`
    // because it can break in unexpected ways if there are pending changes in this.viewsChanged.
    // This function could first check if `this.viewsChanged` has a view for `index` and commit it,
    // but that would be pretty slow, and the same result can be achieved with
    // `this.getReadonly(index).toValue()`
    /**
     * Set Composite element type `view` at `index`
     */
    set(index, view) {
        if (index >= this._length) {
            throw Error(`Error setting index over length ${index} > ${this._length}`);
        }
        // When setting a view:
        // - Not necessary to commit node
        // - Not necessary to persist cache
        // Just keeping a reference to the view in this.viewsChanged ensures consistency
        this.viewsChanged.set(index, view);
    }
    /**
     * WARNING: Returns all commited changes, if there are any pending changes commit them beforehand
     */
    getAllReadonly() {
        this.populateAllNodes();
        const views = new Array(this._length);
        for (let i = 0; i < this._length; i++) {
            views[i] = this.type.elementType.getViewDU(this.nodes[i], this.caches[i]);
        }
        return views;
    }
    /**
     * WARNING: Returns all commited changes, if there are any pending changes commit them beforehand
     */
    getAllReadonlyValues() {
        this.populateAllNodes();
        const values = new Array(this._length);
        for (let i = 0; i < this._length; i++) {
            values[i] = this.type.elementType.tree_toValue(this.nodes[i]);
        }
        return values;
    }
    commit() {
        if (this.viewsChanged.size === 0) {
            return;
        }
        const nodesChanged = [];
        for (const [index, view] of this.viewsChanged) {
            const node = this.type.elementType.commitViewDU(view);
            // Set new node in nodes array to ensure data represented in the tree and fast nodes access is equal
            this.nodes[index] = node;
            nodesChanged.push({ index, node });
            // Cache the view's caches to preserve it's data after 'this.viewsChanged.clear()'
            const cache = this.type.elementType.cacheOfViewDU(view);
            if (cache)
                this.caches[index] = cache;
        }
        // TODO: Optimize to loop only once, Numerical sort ascending
        const nodesChangedSorted = nodesChanged.sort((a, b) => a.index - b.index);
        const indexes = nodesChangedSorted.map((entry) => entry.index);
        const nodes = nodesChangedSorted.map((entry) => entry.node);
        const chunksNode = this.type.tree_getChunksNode(this._rootNode);
        // TODO: Ensure fast setNodesAtDepth() method is correct
        const newChunksNode = persistent_merkle_tree_1.setNodesAtDepth(chunksNode, this.type.chunkDepth, indexes, nodes);
        this._rootNode = this.type.tree_setChunksNode(this._rootNode, newChunksNode, this.dirtyLength ? this._length : undefined);
        this.viewsChanged.clear();
        this.dirtyLength = false;
    }
    clearCache() {
        this.nodes = [];
        this.caches = [];
        this.nodesPopulated = false;
        // It's not necessary to clear this.viewsChanged since they have no effect on the cache.
        // However preserving _SOME_ caches results in a very unpredictable experience.
        this.viewsChanged.clear();
        // Reset cached length only if it has been mutated
        if (this.dirtyLength) {
            this._length = this.type.tree_getLength(this._rootNode);
            this.dirtyLength = false;
        }
    }
    populateAllNodes() {
        // If there's uncommited changes it may break.
        // this.length can be increased but this._rootNode doesn't have that item
        if (this.viewsChanged.size > 0) {
            throw Error("Must commit changes before reading all nodes");
        }
        if (!this.nodesPopulated) {
            this.nodes = persistent_merkle_tree_1.getNodesAtDepth(this._rootNode, this.type.depth, 0, this.length);
            this.nodesPopulated = true;
        }
    }
}
exports.ArrayCompositeTreeViewDU = ArrayCompositeTreeViewDU;
//# sourceMappingURL=arrayComposite.js.map