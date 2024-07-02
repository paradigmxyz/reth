import { Tree } from "@chainsafe/persistent-merkle-tree";
import { ValueOf } from "../type/abstract";
import { CompositeType, CompositeView, CompositeViewDU } from "../type/composite";
import { ArrayCompositeTreeView, ArrayCompositeType } from "./arrayComposite";
/** Expected API of this View's type. This interface allows to break a recursive dependency between types and views */
export declare type ListCompositeType<ElementType extends CompositeType<unknown, CompositeView<ElementType>, CompositeViewDU<ElementType>>> = ArrayCompositeType<ElementType> & {
    readonly limit: number;
};
export declare class ListCompositeTreeView<ElementType extends CompositeType<ValueOf<ElementType>, CompositeView<ElementType>, CompositeViewDU<ElementType>>> extends ArrayCompositeTreeView<ElementType> {
    readonly type: ListCompositeType<ElementType>;
    protected tree: Tree;
    constructor(type: ListCompositeType<ElementType>, tree: Tree);
    /**
     * Adds one view element at the end of the array and adds 1 to the current Tree length.
     */
    push(view: CompositeView<ElementType>): void;
}
//# sourceMappingURL=listComposite.d.ts.map