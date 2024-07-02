import { Tree } from "@chainsafe/persistent-merkle-tree";
import { ValueOf } from "../type/abstract";
import { BasicType } from "../type/basic";
import { ArrayBasicTreeView, ArrayBasicType } from "./arrayBasic";
/** Expected API of this View's type. This interface allows to break a recursive dependency between types and views */
export declare type ListBasicType<ElementType extends BasicType<unknown>> = ArrayBasicType<ElementType> & {
    readonly limit: number;
};
export declare class ListBasicTreeView<ElementType extends BasicType<unknown>> extends ArrayBasicTreeView<ElementType> {
    readonly type: ListBasicType<ElementType>;
    protected tree: Tree;
    constructor(type: ListBasicType<ElementType>, tree: Tree);
    /**
     * Adds one value element at the end of the array and adds 1 to the current Tree length.
     */
    push(value: ValueOf<ElementType>): void;
}
//# sourceMappingURL=listBasic.d.ts.map