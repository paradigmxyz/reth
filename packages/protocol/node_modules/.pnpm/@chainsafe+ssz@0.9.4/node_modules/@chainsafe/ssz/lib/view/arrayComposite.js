"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ArrayCompositeTreeView = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const abstract_1 = require("./abstract");
class ArrayCompositeTreeView extends abstract_1.TreeView {
    constructor(type, tree) {
        super();
        this.type = type;
        this.tree = tree;
    }
    /**
     * Number of elements in the array. Equal to the Uint32 value of the Tree's length node
     */
    get length() {
        return this.type.tree_getLength(this.tree.rootNode);
    }
    /**
     * Returns the View's Tree rootNode
     */
    get node() {
        return this.tree.rootNode;
    }
    /**
     * Get element at `index`. Returns a view of the Composite element type
     */
    get(index) {
        // TODO: Optimize without bitstring
        const gindex = persistent_merkle_tree_1.toGindexBitstring(this.type.depth, index);
        const subtree = this.tree.getSubtree(gindex);
        return this.type.elementType.getView(subtree);
    }
    /**
     * Get element at `index`. Returns a view of the Composite element type.
     * DOES NOT PROPAGATE CHANGES: use only for reads and to skip parent references.
     */
    getReadonly(index) {
        // TODO: Optimize without bitstring
        const gindex = persistent_merkle_tree_1.toGindexBitstring(this.type.depth, index);
        // tree.getSubtree but without the hook
        const subtree = new persistent_merkle_tree_1.Tree(this.tree.getNode(gindex));
        return this.type.elementType.getView(subtree);
    }
    /**
     * Set Composite element type `view` at `index`
     */
    set(index, view) {
        const length = this.length;
        if (index >= length) {
            throw Error(`Error setting index over length ${index} > ${length}`);
        }
        const node = this.type.elementType.commitView(view);
        this.tree.setNodeAtDepth(this.type.depth, index, node);
    }
    /**
     * Returns an array of views of all elements in the array, from index zero to `this.length - 1`.
     * The returned views don't have a parent hook to this View's Tree, so changes in the returned views won't be
     * propagated upwards. To get linked element Views use `this.get()`
     */
    getAllReadonly() {
        const length = this.length;
        const chunksNode = this.type.tree_getChunksNode(this.node);
        const nodes = persistent_merkle_tree_1.getNodesAtDepth(chunksNode, this.type.chunkDepth, 0, length);
        const views = new Array(length);
        for (let i = 0; i < length; i++) {
            // TODO: Optimize
            views[i] = this.type.elementType.getView(new persistent_merkle_tree_1.Tree(nodes[i]));
        }
        return views;
    }
    /**
     * Returns an array of values of all elements in the array, from index zero to `this.length - 1`.
     * The returned values are not Views so any changes won't be propagated upwards.
     * To get linked element Views use `this.get()`
     */
    getAllReadonlyValues() {
        const length = this.length;
        const chunksNode = this.type.tree_getChunksNode(this.node);
        const nodes = persistent_merkle_tree_1.getNodesAtDepth(chunksNode, this.type.chunkDepth, 0, length);
        const values = new Array(length);
        for (let i = 0; i < length; i++) {
            values[i] = this.type.elementType.tree_toValue(nodes[i]);
        }
        return values;
    }
}
exports.ArrayCompositeTreeView = ArrayCompositeTreeView;
//# sourceMappingURL=arrayComposite.js.map