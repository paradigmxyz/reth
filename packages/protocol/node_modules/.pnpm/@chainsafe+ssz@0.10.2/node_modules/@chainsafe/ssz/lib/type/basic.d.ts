import { LeafNode } from "@chainsafe/persistent-merkle-tree";
import { Type } from "./abstract";
/**
 * Represents a basic type as defined in the spec:
 * https://github.com/ethereum/consensus-specs/blob/dev/ssz/simple-serialize.md#basic-types
 */
export declare abstract class BasicType<V> extends Type<V> {
    readonly isBasic = true;
    readonly depth = 0;
    readonly maxChunkCount = 1;
    abstract readonly byteLength: number;
    value_serializedSize(): number;
    tree_serializedSize(): number;
    protected assertValidSize(size: number): void;
    hashTreeRoot(value: V): Uint8Array;
    clone(value: V): V;
    equals(a: V, b: V): boolean;
    /** INTERNAL METHOD: Efficiently get a value from a LeafNode (not packed) */
    abstract tree_getFromNode(leafNode: LeafNode): V;
    /** INTERNAL METHOD: Efficiently set a value to a LeafNode (not packed) */
    abstract tree_setToNode(leafNode: LeafNode, value: V): void;
    /** INTERNAL METHOD: Efficiently get a value from a LeafNode (packed) */
    abstract tree_getFromPackedNode(leafNode: LeafNode, index: number): V;
    /** INTERNAL METHOD: Efficiently set a value to a LeafNode (packed) */
    abstract tree_setToPackedNode(leafNode: LeafNode, index: number, value: V): void;
}
export declare function isBasicType<T>(type: Type<T>): type is BasicType<T>;
//# sourceMappingURL=basic.d.ts.map