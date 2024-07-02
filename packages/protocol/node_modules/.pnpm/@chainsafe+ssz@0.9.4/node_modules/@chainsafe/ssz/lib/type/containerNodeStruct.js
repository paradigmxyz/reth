"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ContainerNodeStructType = void 0;
const composite_1 = require("./composite");
const container_1 = require("./container");
const named_1 = require("../util/named");
const containerNodeStruct_1 = require("../view/containerNodeStruct");
const containerNodeStruct_2 = require("../viewDU/containerNodeStruct");
const branchNodeStruct_1 = require("../branchNodeStruct");
/**
 * ContainerNodeStruct: ordered heterogeneous collection of values.
 * - Notation: Custom name per instance
 *
 * A ContainerNodeStruct is identical to a Container type except that it represents tree data with a custom
 * BranchNodeStruct node. This special branch node represents the data of its entire sub tree as a value, instead
 * of a tree of nodes. This approach is a tradeoff:
 *
 * - More memory efficient
 * - Faster reads, since it doesn't require parsing merkleized data
 * - Slower hashing, since it has to merkleize the entire value everytime and has not intermediary hashing cache
 *
 * This tradeoff is good for data that is read often, written rarely, and consumes a lot of memory (i.e. Validator)
 */
class ContainerNodeStructType extends container_1.ContainerType {
    constructor(fields, opts) {
        super(fields, {
            // Overwrite default "Container" typeName
            // Render detailed typeName. Consumers should overwrite since it can get long
            typeName: opts?.typeName ?? container_1.renderContainerTypeName(fields, "ContainerNodeStruct"),
            ...opts,
            getContainerTreeViewClass: containerNodeStruct_1.getContainerTreeViewClass,
            getContainerTreeViewDUClass: containerNodeStruct_2.getContainerTreeViewDUClass,
        });
        this.fields = fields;
        // ContainerNodeStructType TreeViews don't handle recursive mutable TreeViews like ContainerType does.
        // Using ContainerNodeStructType for fields that have mutable views (like a ListBasic), will result in
        // unnexpected behaviour if those child views are mutated.
        //
        // For example, this example below won't persist the pushed values to the list:
        // ```ts
        // const type = ContainerNodeStructType({a: new ListBasicType(byteType, 1)});
        // const view = type.defaultViewDU();
        // view.a.push(0)
        // ```
        // because the ListBasicViewDU in view.a will never propagate the changes upwards to its ContainerNodeStructType.
        for (const { fieldName, fieldType } of this.fieldsEntries) {
            if (composite_1.isCompositeType(fieldType) && fieldType.isViewMutable) {
                throw Error(`ContainerNodeStructType field '${fieldName}' ${fieldType.typeName} view is mutable`);
            }
        }
    }
    static named(fields, opts) {
        return new (named_1.namedClass(container_1.ContainerType, opts.typeName))(fields, opts);
    }
    tree_serializedSize(node) {
        return this.value_serializedSize(node.value);
    }
    tree_serializeToBytes(output, offset, node) {
        const { value } = node;
        return this.value_serializeToBytes(output, offset, value);
    }
    tree_deserializeFromBytes(data, start, end) {
        const value = this.value_deserializeFromBytes(data, start, end);
        return new branchNodeStruct_1.BranchNodeStruct(this.valueToTree.bind(this), value);
    }
    // Proofs
    // ContainerNodeStructType can only parse proofs that contain all the data.
    // TODO: Support converting a partial tree to a partial value
    getPropertyGindex() {
        return null;
    }
    // Post process tree to convert regular BranchNode to BranchNodeStruct
    // TODO: Optimize conversions
    tree_fromProofNode(node) {
        // TODO: Figure out from `node` alone if it contains complete data.
        // Otherwise throw a nice error "ContainerNodeStruct type requires proofs for all its data"
        const uint8Array = new Uint8Array(super.tree_serializedSize(node));
        const dataView = new DataView(uint8Array.buffer, uint8Array.byteOffset, uint8Array.byteLength);
        super.tree_serializeToBytes({ uint8Array, dataView }, 0, node);
        const value = this.value_deserializeFromBytes({ uint8Array, dataView }, 0, uint8Array.length);
        return {
            node: new branchNodeStruct_1.BranchNodeStruct(this.valueToTree.bind(this), value),
            done: true,
        };
    }
    // Overwrites for fast conversion node <-> value
    tree_toValue(node) {
        return node.value;
    }
    value_toTree(value) {
        return new branchNodeStruct_1.BranchNodeStruct(this.valueToTree.bind(this), value);
    }
    // TODO: Optimize conversion
    valueToTree(value) {
        const uint8Array = new Uint8Array(this.value_serializedSize(value));
        const dataView = new DataView(uint8Array.buffer, uint8Array.byteOffset, uint8Array.byteLength);
        this.value_serializeToBytes({ uint8Array, dataView }, 0, value);
        return super.tree_deserializeFromBytes({ uint8Array, dataView }, 0, uint8Array.length);
    }
}
exports.ContainerNodeStructType = ContainerNodeStructType;
//# sourceMappingURL=containerNodeStruct.js.map