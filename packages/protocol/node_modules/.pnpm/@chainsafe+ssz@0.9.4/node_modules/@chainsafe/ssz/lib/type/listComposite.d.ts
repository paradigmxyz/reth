import { Node, Tree } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { ValueOf, ByteViews } from "./abstract";
import { CompositeType, CompositeView, CompositeViewDU } from "./composite";
import { ArrayCompositeType } from "../view/arrayComposite";
import { ListCompositeTreeView } from "../view/listComposite";
import { ListCompositeTreeViewDU } from "../viewDU/listComposite";
import { ArrayType } from "./array";
export interface ListCompositeOpts {
    typeName?: string;
}
/**
 * List: ordered variable-length homogeneous collection, limited to N values
 *
 * Array of Composite type:
 * - Composite types always take at least one chunk
 * - Composite types are always returned as views
 */
export declare class ListCompositeType<ElementType extends CompositeType<any, CompositeView<ElementType>, CompositeViewDU<ElementType>>> extends ArrayType<ElementType, ListCompositeTreeView<ElementType>, ListCompositeTreeViewDU<ElementType>> implements ArrayCompositeType<ElementType> {
    readonly elementType: ElementType;
    readonly limit: number;
    readonly typeName: string;
    readonly itemsPerChunk = 1;
    readonly depth: number;
    readonly chunkDepth: number;
    readonly maxChunkCount: number;
    readonly fixedSize: null;
    readonly minSize: number;
    readonly maxSize: number;
    readonly isList = true;
    readonly isViewMutable = true;
    protected readonly defaultLen = 0;
    constructor(elementType: ElementType, limit: number, opts?: ListCompositeOpts);
    static named<ElementType extends CompositeType<any, CompositeView<ElementType>, CompositeViewDU<ElementType>>>(elementType: ElementType, limit: number, opts: Require<ListCompositeOpts, "typeName">): ListCompositeType<ElementType>;
    getView(tree: Tree): ListCompositeTreeView<ElementType>;
    getViewDU(node: Node, cache?: unknown): ListCompositeTreeViewDU<ElementType>;
    commitView(view: ListCompositeTreeView<ElementType>): Node;
    commitViewDU(view: ListCompositeTreeViewDU<ElementType>): Node;
    cacheOfViewDU(view: ListCompositeTreeViewDU<ElementType>): unknown;
    value_serializedSize(value: ValueOf<ElementType>[]): number;
    value_serializeToBytes(output: ByteViews, offset: number, value: ValueOf<ElementType>[]): number;
    value_deserializeFromBytes(data: ByteViews, start: number, end: number): ValueOf<ElementType>[];
    tree_serializedSize(node: Node): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getLength(node: Node): number;
    tree_setLength(tree: Tree, length: number): void;
    tree_getChunksNode(node: Node): Node;
    tree_setChunksNode(rootNode: Node, chunksNode: Node, newLength?: number): Node;
    hashTreeRoot(value: ValueOf<ElementType>[]): Uint8Array;
    protected getRoots(value: ValueOf<ElementType>[]): Uint8Array[];
}
//# sourceMappingURL=listComposite.d.ts.map