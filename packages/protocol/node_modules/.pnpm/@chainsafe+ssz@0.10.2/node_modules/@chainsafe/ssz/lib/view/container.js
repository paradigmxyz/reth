"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.getContainerTreeViewClass = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const basic_1 = require("../type/basic");
const composite_1 = require("../type/composite");
const abstract_1 = require("./abstract");
/**
 * Intented usage:
 *
 * - Get initial BeaconState from disk.
 * - Before applying next block, switch to mutable
 * - Get some field, create a view in mutable mode
 * - Do modifications of the state in the state transition function
 * - When done, commit and apply new root node once to og BeaconState
 * - However, keep all the caches and transfer them to the new BeaconState
 *
 * Questions:
 * - Can the child views created in mutable mode switch to not mutable? If so, it seems that it needs to recursively
 *   iterate the entire data structure and views
 *
 */
class ContainerTreeView extends abstract_1.TreeView {
    constructor(type, tree) {
        super();
        this.type = type;
        this.tree = tree;
    }
    get node() {
        return this.tree.rootNode;
    }
}
function getContainerTreeViewClass(type) {
    class CustomContainerTreeView extends ContainerTreeView {
    }
    // Dynamically define prototype methods
    for (let index = 0; index < type.fieldsEntries.length; index++) {
        const { fieldName, fieldType } = type.fieldsEntries[index];
        // If the field type is basic, the value to get and set will be the actual 'struct' value (i.e. a JS number).
        // The view must use the tree_getFromNode() and tree_setToNode() methods to persist the struct data to the node,
        // and use the cached views array to store the new node.
        if (basic_1.isBasicType(fieldType)) {
            Object.defineProperty(CustomContainerTreeView.prototype, fieldName, {
                configurable: false,
                enumerable: true,
                // TODO: Review the memory cost of this closures
                get: function () {
                    const leafNode = persistent_merkle_tree_1.getNodeAtDepth(this.node, this.type.depth, index);
                    return fieldType.tree_getFromNode(leafNode);
                },
                set: function (value) {
                    const leafNodePrev = persistent_merkle_tree_1.getNodeAtDepth(this.node, this.type.depth, index);
                    const leafNode = leafNodePrev.clone();
                    fieldType.tree_setToNode(leafNode, value);
                    this.tree.setNodeAtDepth(this.type.depth, index, leafNode);
                },
            });
        }
        // If the field type is composite, the value to get and set will be another TreeView. The parent TreeView must
        // cache the view itself to retain the caches of the child view. To set a value the view must return a node to
        // set it to the parent tree in the field gindex.
        else if (composite_1.isCompositeType(fieldType)) {
            Object.defineProperty(CustomContainerTreeView.prototype, fieldName, {
                configurable: false,
                enumerable: true,
                // Returns TreeView of fieldName
                get: function () {
                    const gindex = persistent_merkle_tree_1.toGindexBitstring(this.type.depth, index);
                    return fieldType.getView(this.tree.getSubtree(gindex));
                },
                // Expects TreeView of fieldName
                set: function (value) {
                    const node = fieldType.commitView(value);
                    this.tree.setNodeAtDepth(this.type.depth, index, node);
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
    Object.defineProperty(CustomContainerTreeView, "name", { value: type.typeName, writable: false });
    return CustomContainerTreeView;
}
exports.getContainerTreeViewClass = getContainerTreeViewClass;
//# sourceMappingURL=container.js.map