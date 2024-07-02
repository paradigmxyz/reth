import { Node } from "@chainsafe/persistent-merkle-tree";
import { ValueOf } from "../type/abstract";
import { CompositeType, CompositeView, CompositeViewDU } from "../type/composite";
import { ArrayCompositeType } from "../view/arrayComposite";
import { TreeViewDU } from "./abstract";
export declare type ArrayCompositeTreeViewDUCache = {
    nodes: Node[];
    caches: unknown[];
    length: number;
    nodesPopulated: boolean;
};
export declare class ArrayCompositeTreeViewDU<ElementType extends CompositeType<ValueOf<ElementType>, CompositeView<ElementType>, CompositeViewDU<ElementType>>> extends TreeViewDU<ArrayCompositeType<ElementType>> {
    readonly type: ArrayCompositeType<ElementType>;
    protected _rootNode: Node;
    protected nodes: Node[];
    protected caches: unknown[];
    protected readonly viewsChanged: Map<number, CompositeViewDU<ElementType>>;
    protected _length: number;
    protected dirtyLength: boolean;
    private nodesPopulated;
    constructor(type: ArrayCompositeType<ElementType>, _rootNode: Node, cache?: ArrayCompositeTreeViewDUCache);
    /**
     * Number of elements in the array. Equal to un-commited length of the array
     */
    get length(): number;
    get node(): Node;
    get cache(): ArrayCompositeTreeViewDUCache;
    /**
     * Get element at `index`. Returns a view of the Composite element type.
     *
     * NOTE: Assumes that any view created here will change and will call .commit() on it.
     * .get() should be used only for cases when something may mutate. To get all items without
     * triggering a .commit() in all them use .getAllReadOnly().
     */
    get(index: number): CompositeViewDU<ElementType>;
    /**
     * Get element at `index`. Returns a view of the Composite element type.
     * DOES NOT PROPAGATE CHANGES: use only for reads and to skip parent references.
     */
    getReadonly(index: number): CompositeViewDU<ElementType>;
    /**
     * Set Composite element type `view` at `index`
     */
    set(index: number, view: CompositeViewDU<ElementType>): void;
    /**
     * WARNING: Returns all commited changes, if there are any pending changes commit them beforehand
     */
    getAllReadonly(): CompositeViewDU<ElementType>[];
    /**
     * WARNING: Returns all commited changes, if there are any pending changes commit them beforehand
     */
    getAllReadonlyValues(): ValueOf<ElementType>[];
    commit(): void;
    protected clearCache(): void;
    private populateAllNodes;
}
//# sourceMappingURL=arrayComposite.d.ts.map