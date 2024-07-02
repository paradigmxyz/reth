import { Gindex, Node, Tree } from "@chainsafe/persistent-merkle-tree";
import { ByteViews } from "./abstract";
import { CompositeType } from "./composite";
export declare type ByteArray = Uint8Array;
/**
 * ByteArray: ordered array collection of byte values
 * - Value: `Uint8Array`
 * - View: `Uint8Array`
 * - ViewDU: `Uint8Array`
 *
 * ByteArray is an immutable value which is represented by a Uint8Array for memory efficiency and performance.
 * Note: Consumers of this type MUST never mutate the `Uint8Array` representation of a ByteArray.
 */
export declare abstract class ByteArrayType extends CompositeType<ByteArray, ByteArray, ByteArray> {
    readonly isViewMutable = false;
    defaultValue(): ByteArray;
    getView(tree: Tree): ByteArray;
    getViewDU(node: Node): ByteArray;
    commitView(view: ByteArray): Node;
    commitViewDU(view: ByteArray): Node;
    cacheOfViewDU(): unknown;
    toView(value: ByteArray): ByteArray;
    toViewDU(value: ByteArray): ByteArray;
    value_serializeToBytes(output: ByteViews, offset: number, value: ByteArray): number;
    value_deserializeFromBytes(data: ByteViews, start: number, end: number): ByteArray;
    protected getRoots(value: ByteArray): Uint8Array[];
    getPropertyGindex(): null;
    getPropertyType(): never;
    getIndexProperty(): never;
    tree_fromProofNode(node: Node): {
        node: Node;
        done: boolean;
    };
    tree_getLeafGindices(rootGindex: bigint, rootNode?: Node): Gindex[];
    abstract tree_getByteLen(node?: Node): number;
    fromJson(json: unknown): ByteArray;
    toJson(value: ByteArray): unknown;
    clone(value: ByteArray): ByteArray;
    equals(a: Uint8Array, b: Uint8Array): boolean;
    protected abstract assertValidSize(size: number): void;
}
//# sourceMappingURL=byteArray.d.ts.map