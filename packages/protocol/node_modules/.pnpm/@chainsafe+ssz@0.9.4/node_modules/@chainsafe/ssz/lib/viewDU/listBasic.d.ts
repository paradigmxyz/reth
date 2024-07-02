import { Node } from "@chainsafe/persistent-merkle-tree";
import { ValueOf } from "../type/abstract";
import { BasicType } from "../type/basic";
import { ListBasicType } from "../view/listBasic";
import { ArrayBasicTreeViewDU, ArrayBasicTreeViewDUCache } from "./arrayBasic";
export declare class ListBasicTreeViewDU<ElementType extends BasicType<unknown>> extends ArrayBasicTreeViewDU<ElementType> {
    readonly type: ListBasicType<ElementType>;
    protected _rootNode: Node;
    constructor(type: ListBasicType<ElementType>, _rootNode: Node, cache?: ArrayBasicTreeViewDUCache);
    /**
     * Adds one value element at the end of the array and adds 1 to the un-commited ViewDU length
     */
    push(value: ValueOf<ElementType>): void;
}
//# sourceMappingURL=listBasic.d.ts.map