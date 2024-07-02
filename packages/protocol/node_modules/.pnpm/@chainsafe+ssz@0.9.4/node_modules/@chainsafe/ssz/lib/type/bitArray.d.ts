import { Gindex, Node, Tree } from "@chainsafe/persistent-merkle-tree";
import { CompositeType } from "./composite";
import { BitArray } from "../value/bitArray";
import { BitArrayTreeView } from "../view/bitArray";
import { BitArrayTreeViewDU } from "../viewDU/bitArray";
/**
 * BitArray: ordered array collection of boolean values
 * - Value: `BitArray`, @see BitArray for a justification of its memory efficiency and performance
 * - View: `BitArrayTreeView`
 * - ViewDU: `BitArrayTreeViewDU`
 */
export declare abstract class BitArrayType extends CompositeType<BitArray, BitArrayTreeView, BitArrayTreeViewDU> {
    readonly isViewMutable = true;
    getView(tree: Tree): BitArrayTreeView;
    getViewDU(node: Node): BitArrayTreeViewDU;
    commitView(view: BitArrayTreeView): Node;
    commitViewDU(view: BitArrayTreeViewDU): Node;
    cacheOfViewDU(view: BitArrayTreeViewDU): unknown;
    protected getRoots(value: BitArray): Uint8Array[];
    getPropertyGindex(): null;
    getPropertyType(): never;
    getIndexProperty(): never;
    tree_fromProofNode(node: Node): {
        node: Node;
        done: boolean;
    };
    tree_getLeafGindices(rootGindex: bigint, rootNode?: Node): Gindex[];
    abstract tree_getByteLen(node?: Node): number;
    fromJson(json: unknown): BitArray;
    toJson(value: BitArray): unknown;
    clone(value: BitArray): BitArray;
    equals(a: BitArray, b: BitArray): boolean;
}
//# sourceMappingURL=bitArray.d.ts.map