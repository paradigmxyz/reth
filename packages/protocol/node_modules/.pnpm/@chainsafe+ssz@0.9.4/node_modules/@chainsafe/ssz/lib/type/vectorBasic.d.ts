import { Node, Tree } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { ValueOf, ByteViews } from "./abstract";
import { BasicType } from "./basic";
import { ArrayBasicType, ArrayBasicTreeView } from "../view/arrayBasic";
import { ArrayBasicTreeViewDU } from "../viewDU/arrayBasic";
import { ArrayType } from "./array";
export declare type VectorBasicOpts = {
    typeName?: string;
};
/**
 * Vector: Ordered fixed-length homogeneous collection, with N values
 *
 * Array of Basic type:
 * - Basic types are max 32 bytes long so multiple values may be packed in the same node.
 * - Basic types are never returned in a view wrapper, but their value representation
 */
export declare class VectorBasicType<ElementType extends BasicType<unknown>> extends ArrayType<ElementType, ArrayBasicTreeView<ElementType>, ArrayBasicTreeViewDU<ElementType>> implements ArrayBasicType<ElementType> {
    readonly elementType: ElementType;
    readonly length: number;
    readonly typeName: string;
    readonly itemsPerChunk: number;
    readonly depth: number;
    readonly chunkDepth: number;
    readonly maxChunkCount: number;
    readonly fixedSize: number;
    readonly minSize: number;
    readonly maxSize: number;
    readonly isList = false;
    readonly isViewMutable = true;
    protected readonly defaultLen: number;
    constructor(elementType: ElementType, length: number, opts?: VectorBasicOpts);
    static named<ElementType extends BasicType<unknown>>(elementType: ElementType, limit: number, opts: Require<VectorBasicOpts, "typeName">): VectorBasicType<ElementType>;
    getView(tree: Tree): ArrayBasicTreeView<ElementType>;
    getViewDU(node: Node, cache?: unknown): ArrayBasicTreeViewDU<ElementType>;
    commitView(view: ArrayBasicTreeView<ElementType>): Node;
    commitViewDU(view: ArrayBasicTreeViewDU<ElementType>): Node;
    cacheOfViewDU(view: ArrayBasicTreeViewDU<ElementType>): unknown;
    value_serializedSize(): number;
    value_serializeToBytes(output: ByteViews, offset: number, value: ValueOf<ElementType>[]): number;
    value_deserializeFromBytes(data: ByteViews, start: number, end: number): ValueOf<ElementType>[];
    tree_serializedSize(): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getLength(): number;
    tree_setLength(): void;
    tree_getChunksNode(node: Node): Node;
    tree_setChunksNode(rootNode: Node, chunksNode: Node): Node;
    protected getRoots(value: ValueOf<ElementType>[]): Uint8Array[];
}
//# sourceMappingURL=vectorBasic.d.ts.map