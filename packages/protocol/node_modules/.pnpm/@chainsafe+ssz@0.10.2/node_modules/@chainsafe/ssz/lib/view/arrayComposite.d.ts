import { Node, Tree } from "@chainsafe/persistent-merkle-tree";
import { ValueOf } from "../type/abstract";
import { CompositeType, CompositeView, CompositeViewDU } from "../type/composite";
import { TreeView } from "./abstract";
/** Expected API of this View's type. This interface allows to break a recursive dependency between types and views */
export declare type ArrayCompositeType<ElementType extends CompositeType<unknown, CompositeView<ElementType>, CompositeViewDU<ElementType>>> = CompositeType<ValueOf<ElementType>[], unknown, unknown> & {
    readonly elementType: ElementType;
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
export declare class ArrayCompositeTreeView<ElementType extends CompositeType<ValueOf<ElementType>, CompositeView<ElementType>, CompositeViewDU<ElementType>>> extends TreeView<ArrayCompositeType<ElementType>> {
    readonly type: ArrayCompositeType<ElementType>;
    protected tree: Tree;
    constructor(type: ArrayCompositeType<ElementType>, tree: Tree);
    /**
     * Number of elements in the array. Equal to the Uint32 value of the Tree's length node
     */
    get length(): number;
    /**
     * Returns the View's Tree rootNode
     */
    get node(): Node;
    /**
     * Get element at `index`. Returns a view of the Composite element type
     */
    get(index: number): CompositeView<ElementType>;
    /**
     * Get element at `index`. Returns a view of the Composite element type.
     * DOES NOT PROPAGATE CHANGES: use only for reads and to skip parent references.
     */
    getReadonly(index: number): CompositeView<ElementType>;
    /**
     * Set Composite element type `view` at `index`
     */
    set(index: number, view: CompositeView<ElementType>): void;
    /**
     * Returns an array of views of all elements in the array, from index zero to `this.length - 1`.
     * The returned views don't have a parent hook to this View's Tree, so changes in the returned views won't be
     * propagated upwards. To get linked element Views use `this.get()`
     */
    getAllReadonly(): CompositeView<ElementType>[];
    /**
     * Returns an array of values of all elements in the array, from index zero to `this.length - 1`.
     * The returned values are not Views so any changes won't be propagated upwards.
     * To get linked element Views use `this.get()`
     */
    getAllReadonlyValues(): ValueOf<ElementType>[];
}
//# sourceMappingURL=arrayComposite.d.ts.map