"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.getContainerTreeViewDUClass = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const basic_1 = require("../type/basic");
const composite_1 = require("../type/composite");
const abstract_1 = require("./abstract");
class ContainerTreeViewDU extends abstract_1.TreeViewDU {
    constructor(type, _rootNode, cache) {
        super();
        this.type = type;
        this._rootNode = _rootNode;
        this.nodes = [];
        this.nodesChanged = new Set();
        this.viewsChanged = new Map();
        if (cache) {
            this.nodes = cache.nodes;
            this.caches = cache.caches;
            this.nodesPopulated = cache.nodesPopulated;
        }
        else {
            this.nodes = [];
            this.caches = [];
            this.nodesPopulated = false;
        }
    }
    get node() {
        return this._rootNode;
    }
    get cache() {
        return {
            nodes: this.nodes,
            caches: this.caches,
            nodesPopulated: this.nodesPopulated,
        };
    }
    commit() {
        if (this.nodesChanged.size === 0 && this.viewsChanged.size === 0) {
            return;
        }
        const nodesChanged = [];
        for (const [index, view] of this.viewsChanged) {
            const fieldType = this.type.fieldsEntries[index].fieldType;
            const node = fieldType.commitViewDU(view);
            // Set new node in nodes array to ensure data represented in the tree and fast nodes access is equal
            this.nodes[index] = node;
            nodesChanged.push({ index, node });
            // Cache the view's caches to preserve it's data after 'this.viewsChanged.clear()'
            const cache = fieldType.cacheOfViewDU(view);
            if (cache)
                this.caches[index] = cache;
        }
        for (const index of this.nodesChanged) {
            nodesChanged.push({ index, node: this.nodes[index] });
        }
        // TODO: Optimize to loop only once, Numerical sort ascending
        const nodesChangedSorted = nodesChanged.sort((a, b) => a.index - b.index);
        const indexes = nodesChangedSorted.map((entry) => entry.index);
        const nodes = nodesChangedSorted.map((entry) => entry.node);
        this._rootNode = persistent_merkle_tree_1.setNodesAtDepth(this._rootNode, this.type.depth, indexes, nodes);
        this.nodesChanged.clear();
        this.viewsChanged.clear();
    }
    clearCache() {
        this.nodes = [];
        this.caches = [];
        this.nodesPopulated = false;
        // Must clear nodesChanged, otherwise a subsequent commit call will break, because it assumes a node is there
        this.nodesChanged.clear();
        // It's not necessary to clear this.viewsChanged since they have no effect on the cache.
        // However preserving _SOME_ caches results in a very unpredictable experience.
        this.viewsChanged.clear();
    }
}
function getContainerTreeViewDUClass(type) {
    class CustomContainerTreeViewDU extends ContainerTreeViewDU {
    }
    // Dynamically define prototype methods
    for (let index = 0; index < type.fieldsEntries.length; index++) {
        const { fieldName, fieldType } = type.fieldsEntries[index];
        // If the field type is basic, the value to get and set will be the actual 'struct' value (i.e. a JS number).
        // The view must use the tree_getFromNode() and tree_setToNode() methods to persist the struct data to the node,
        // and use the cached views array to store the new node.
        if (basic_1.isBasicType(fieldType)) {
            Object.defineProperty(CustomContainerTreeViewDU.prototype, fieldName, {
                configurable: false,
                enumerable: true,
                // TODO: Review the memory cost of this closures
                get: function () {
                    // First walk through the tree to get the root node for that index
                    let node = this.nodes[index];
                    if (node === undefined) {
                        node = persistent_merkle_tree_1.getNodeAtDepth(this._rootNode, this.type.depth, index);
                        this.nodes[index] = node;
                    }
                    return fieldType.tree_getFromNode(node);
                },
                set: function (value) {
                    // Create new node if current leafNode is not dirty
                    let nodeChanged;
                    if (this.nodesChanged.has(index)) {
                        // TODO: This assumes that node has already been populated
                        nodeChanged = this.nodes[index];
                    }
                    else {
                        const nodePrev = (this.nodes[index] ?? persistent_merkle_tree_1.getNodeAtDepth(this._rootNode, this.type.depth, index));
                        nodeChanged = nodePrev.clone();
                        // Store the changed node in the nodes cache
                        this.nodes[index] = nodeChanged;
                        this.nodesChanged.add(index);
                    }
                    fieldType.tree_setToNode(nodeChanged, value);
                },
            });
        }
        // If the field type is composite, the value to get and set will be another TreeView. The parent TreeView must
        // cache the view itself to retain the caches of the child view. To set a value the view must return a node to
        // set it to the parent tree in the field gindex.
        else if (composite_1.isCompositeType(fieldType)) {
            Object.defineProperty(CustomContainerTreeViewDU.prototype, fieldName, {
                configurable: false,
                enumerable: true,
                // Returns TreeViewDU of fieldName
                get: function () {
                    const viewChanged = this.viewsChanged.get(index);
                    if (viewChanged) {
                        // eslint-disable-next-line @typescript-eslint/no-unsafe-return
                        return viewChanged;
                    }
                    let node = this.nodes[index];
                    if (node === undefined) {
                        node = persistent_merkle_tree_1.getNodeAtDepth(this._rootNode, this.type.depth, index);
                        this.nodes[index] = node;
                    }
                    // Keep a reference to the new view to call .commit on it latter, only if mutable
                    const view = fieldType.getViewDU(node, this.caches[index]);
                    if (fieldType.isViewMutable) {
                        this.viewsChanged.set(index, view);
                    }
                    // No need to persist the child's view cache since a second get returns this view instance.
                    // The cache is only persisted on commit where the viewsChanged map is dropped.
                    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
                    return view;
                },
                // Expects TreeViewDU of fieldName
                set: function (view) {
                    // When setting a view:
                    // - Not necessary to commit node
                    // - Not necessary to persist cache
                    // Just keeping a reference to the view in this.viewsChanged ensures consistency
                    this.viewsChanged.set(index, view);
                },
            });
        }
        // Should never happen
        else {
            /* istanbul ignore next - unreachable code */
            throw Error(`Unknown fieldType ${fieldType.typeName} for fieldName ${fieldName}`);
        }
    }
    // Change class name
    Object.defineProperty(CustomContainerTreeViewDU, "name", { value: type.typeName, writable: false });
    return CustomContainerTreeViewDU;
}
exports.getContainerTreeViewDUClass = getContainerTreeViewDUClass;
//# sourceMappingURL=container.js.map