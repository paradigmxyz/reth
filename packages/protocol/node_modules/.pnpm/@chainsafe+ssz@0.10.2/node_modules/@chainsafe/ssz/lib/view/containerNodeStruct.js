"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.getContainerTreeViewClass = void 0;
const composite_1 = require("../type/composite");
const branchNodeStruct_1 = require("../branchNodeStruct");
const abstract_1 = require("./abstract");
/* eslint-disable @typescript-eslint/member-ordering */
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
        if (fieldType.isBasic) {
            Object.defineProperty(CustomContainerTreeView.prototype, fieldName, {
                configurable: false,
                enumerable: true,
                // TODO: Review the memory cost of this closures
                get: function () {
                    return this.tree.rootNode.value[fieldName];
                },
                set: function (value) {
                    const node = this.tree.rootNode;
                    const newNodeValue = this.type.clone(node.value);
                    // TODO: Should this check for valid field name? Benchmark the cost
                    newNodeValue[fieldName] = value;
                    this.tree.rootNode = new branchNodeStruct_1.BranchNodeStruct(node["valueToNode"], newNodeValue);
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
                    const { value } = this.tree.rootNode;
                    return fieldType.toView(value[fieldName]);
                },
                // Expects TreeView of fieldName
                set: function (view) {
                    const node = this.tree.rootNode;
                    const newNodeValue = this.type.clone(node.value);
                    // TODO: Should this check for valid field name? Benchmark the cost
                    newNodeValue[fieldName] = fieldType.toValueFromView(view);
                    this.tree.rootNode = new branchNodeStruct_1.BranchNodeStruct(node["valueToNode"], newNodeValue);
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
//# sourceMappingURL=containerNodeStruct.js.map