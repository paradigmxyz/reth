import { Node, Tree } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { ValueOf, ByteViews } from "./abstract";
import { CompositeType, CompositeView, CompositeViewDU } from "./composite";
import { ArrayCompositeType, ArrayCompositeTreeView } from "../view/arrayComposite";
import { ArrayCompositeTreeViewDU } from "../viewDU/arrayComposite";
import { ArrayType } from "./array";
export declare type VectorCompositeOpts = {
    typeName?: string;
};
/**
 * Vector: Ordered fixed-length homogeneous collection, with N values
 *
 * Array of Composite type:
 * - Composite types always take at least one chunk
 * - Composite types are always returned as views
 */
export declare class VectorCompositeType<ElementType extends CompositeType<any, CompositeView<ElementType>, CompositeViewDU<ElementType>>> extends ArrayType<ElementType, ArrayCompositeTreeView<ElementType>, ArrayCompositeTreeViewDU<ElementType>> implements ArrayCompositeType<ElementType> {
    readonly elementType: ElementType;
    readonly length: number;
    readonly typeName: string;
    readonly itemsPerChunk = 1;
    readonly depth: number;
    readonly chunkDepth: number;
    readonly maxChunkCount: number;
    readonly fixedSize: number | null;
    readonly minSize: number;
    readonly maxSize: number;
    readonly isList = false;
    readonly isViewMutable = true;
    protected readonly defaultLen: number;
    constructor(elementType: ElementType, length: number, opts?: VectorCompositeOpts);
    static named<ElementType extends CompositeType<any, CompositeView<ElementType>, CompositeViewDU<ElementType>>>(elementType: ElementType, limit: number, opts: Require<VectorCompositeOpts, "typeName">): VectorCompositeType<ElementType>;
    getView(tree: Tree): ArrayCompositeTreeView<ElementType>;
    getViewDU(node: Node, cache?: unknown): ArrayCompositeTreeViewDU<ElementType>;
    commitView(view: ArrayCompositeTreeView<ElementType>): Node;
    commitViewDU(view: ArrayCompositeTreeViewDU<ElementType>): Node;
    cacheOfViewDU(view: ArrayCompositeTreeViewDU<ElementType>): unknown;
    value_serializedSize(value: ValueOf<ElementType>[]): number;
    value_serializeToBytes(output: ByteViews, offset: number, value: ValueOf<ElementType>[]): number;
    value_deserializeFromBytes(data: ByteViews, start: number, end: number): ValueOf<ElementType>[];
    tree_serializedSize(node: Node): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getLength(): number;
    tree_setLength(): void;
    tree_getChunksNode(node: Node): Node;
    tree_setChunksNode(rootNode: Node, chunksNode: Node): Node;
    protected getRoots(value: ValueOf<ElementType>[]): Uint8Array[];
}
//# sourceMappingURL=vectorComposite.d.ts.map