import { LeafNode, Node } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { ByteViews } from "./abstract";
import { BasicType } from "./basic";
export interface BooleanOpts {
    typeName?: string;
}
/**
 * Boolean: True or False
 * - Notation: `boolean`
 */
export declare class BooleanType extends BasicType<boolean> {
    readonly typeName: string;
    readonly byteLength = 1;
    readonly itemsPerChunk = 32;
    readonly fixedSize = 1;
    readonly minSize = 1;
    readonly maxSize = 1;
    constructor(opts?: BooleanOpts);
    static named(opts: Require<BooleanOpts, "typeName">): BooleanType;
    defaultValue(): boolean;
    value_serializeToBytes(output: ByteViews, offset: number, value: boolean): number;
    value_deserializeFromBytes(data: ByteViews, start: number, end: number): boolean;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getFromNode(leafNode: LeafNode): boolean;
    tree_setToNode(leafNode: LeafNode, value: boolean): void;
    tree_getFromPackedNode(leafNode: LeafNode, index: number): boolean;
    tree_setToPackedNode(leafNode: LeafNode, index: number, value: boolean): void;
    fromJson(json: unknown): boolean;
    toJson(value: boolean): unknown;
}
//# sourceMappingURL=boolean.d.ts.map