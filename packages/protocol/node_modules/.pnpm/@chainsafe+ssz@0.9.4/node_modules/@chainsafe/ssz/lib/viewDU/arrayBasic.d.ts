import { LeafNode, Node } from "@chainsafe/persistent-merkle-tree";
import { ValueOf } from "../type/abstract";
import { BasicType } from "../type/basic";
import { ArrayBasicType } from "../view/arrayBasic";
import { TreeViewDU } from "./abstract";
export declare type ArrayBasicTreeViewDUCache = {
    nodes: LeafNode[];
    length: number;
    nodesPopulated: boolean;
};
export declare class ArrayBasicTreeViewDU<ElementType extends BasicType<unknown>> extends TreeViewDU<ArrayBasicType<ElementType>> {
    readonly type: ArrayBasicType<ElementType>;
    protected _rootNode: Node;
    protected nodes: LeafNode[];
    protected readonly nodesChanged: Set<number>;
    protected _length: number;
    protected dirtyLength: boolean;
    private nodesPopulated;
    constructor(type: ArrayBasicType<ElementType>, _rootNode: Node, cache?: ArrayBasicTreeViewDUCache);
    /**
     * Number of elements in the array. Equal to un-commited length of the array
     */
    get length(): number;
    get node(): Node;
    get cache(): ArrayBasicTreeViewDUCache;
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
    commit(): void;
    protected clearCache(): void;
}
//# sourceMappingURL=arrayBasic.d.ts.map