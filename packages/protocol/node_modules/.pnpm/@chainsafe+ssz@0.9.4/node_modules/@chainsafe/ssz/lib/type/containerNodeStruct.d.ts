import { Node } from "@chainsafe/persistent-merkle-tree";
import { Type, ByteViews } from "./abstract";
import { ContainerType, ContainerOptions } from "./container";
import { Require } from "../util/types";
import { ValueOfFields } from "../view/container";
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
export declare class ContainerNodeStructType<Fields extends Record<string, Type<unknown>>> extends ContainerType<Fields> {
    readonly fields: Fields;
    constructor(fields: Fields, opts?: ContainerOptions<Fields>);
    static named<Fields extends Record<string, Type<unknown>>>(fields: Fields, opts: Require<ContainerOptions<Fields>, "typeName">): ContainerType<Fields>;
    tree_serializedSize(node: Node): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    getPropertyGindex(): null;
    tree_fromProofNode(node: Node): {
        node: Node;
        done: boolean;
    };
    tree_toValue(node: Node): ValueOfFields<Fields>;
    value_toTree(value: ValueOfFields<Fields>): Node;
    private valueToTree;
}
//# sourceMappingURL=containerNodeStruct.d.ts.map