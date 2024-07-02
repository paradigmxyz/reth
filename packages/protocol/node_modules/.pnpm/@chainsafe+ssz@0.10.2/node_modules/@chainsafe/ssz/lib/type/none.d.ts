import { LeafNode, Node } from "@chainsafe/persistent-merkle-tree";
import { ByteViews } from "./abstract";
import { BasicType } from "./basic";
export declare class NoneType extends BasicType<null> {
    readonly typeName = "none";
    readonly byteLength = 0;
    readonly itemsPerChunk = 32;
    readonly fixedSize = 0;
    readonly minSize = 0;
    readonly maxSize = 0;
    defaultValue(): null;
    value_serializeToBytes(output: ByteViews, offset: number, value: null): number;
    value_deserializeFromBytes(data: ByteViews, start: number): null;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getFromNode(leafNode: LeafNode): null;
    tree_setToNode(leafNode: LeafNode, value: null): void;
    tree_getFromPackedNode(leafNode: LeafNode, index: number): null;
    tree_setToPackedNode(leafNode: LeafNode, index: number, value: null): void;
    fromJson(json: unknown): null;
    toJson(value: null): unknown;
}
//# sourceMappingURL=none.d.ts.map