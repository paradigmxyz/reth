import { Node, Tree } from "@chainsafe/persistent-merkle-tree";
import { ValueOf } from "../type/abstract";
import { BasicType } from "../type/basic";
import { CompositeType } from "../type/composite";
import { TreeViewDU } from "../viewDU/abstract";
import { TreeView } from "./abstract";
/** Expected API of this View's type. This interface allows to break a recursive dependency between types and views */
export declare type ArrayBasicType<ElementType extends BasicType<unknown>> = CompositeType<ValueOf<ElementType>[], TreeView<ArrayBasicType<ElementType>>, TreeViewDU<ArrayBasicType<ElementType>>> & {
    readonly elementType: ElementType;
    readonly itemsPerChunk: number;
    readonly chunkDepth: number;
    /** INTERNAL METHOD: Return the length of this type from an Array's root node */
    tree_getLength(node: Node): number;
    /** INTERNAL METHOD: Mutate a tree's rootNode with a new length value */
    tree_setLength(tree: Tree, length: number): void;
    /** INTERNAL METHOD: Return the chunks node from a root node */
    tree_getChunksNode(rootNode: Node): Node;
    /** INTERNAL METHOD: Return a new root node with changed chunks node and length */
    tree_setChunksNode(rootNode: Node, chunksNode: Node, newLength?: number): Node;
};
export declare class ArrayBasicTreeView<ElementType extends BasicType<unknown>> extends TreeView<ArrayBasicType<ElementType>> {
    readonly type: ArrayBasicType<ElementType>;
    protected tree: Tree;
    constructor(type: ArrayBasicType<ElementType>, tree: Tree);
    /**
     * Number of elements in the array. Equal to the Uint32 value of the Tree's length node
     */
    get length(): number;
    get node(): Node;
    /**
     * Get element at `index`. Returns the Basic element type value directly
     */
    get(index: number): ValueOf<ElementType>;
    /**
     * Set Basic element type `value` at `index`
     */
    set(index: number, value: ValueOf<ElementType>): void;
    /**
     * Get all values of this array as Basic element type values, from index zero to `this.length - 1`
     */
    getAll(): ValueOf<ElementType>[];
}
//# sourceMappingURL=arrayBasic.d.ts.map