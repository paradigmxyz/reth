import { Node } from "@chainsafe/persistent-merkle-tree";
export declare type ValueOf<T extends Type<unknown>> = T extends Type<infer V> ? V : never;
/**
 * JSON path property
 * @example Container property
 * ```
 * "validators"
 * ```
 * @example Array index
 * ```
 * 1234
 * ```
 */
export declare type JsonPathProp = string | number;
/**
 * JSON Proof path
 * @example
 * ```
 * ["validators", 1234, "slashed"]
 * ```
 */
export declare type JsonPath = JsonPathProp[];
/**
 * Provide two views recursively to any deserialization operation:
 * - For uint it's x10 times faster to read and write with DataView
 * - For ByteArray and BitArray it's x10 times faster to slice a Uint8Array than an ArrayBuffer
 *
 * Providing both allows to optimize for both cases with the tiny overhead of creating a new view.
 */
export declare type ByteViews = {
    uint8Array: Uint8Array;
    dataView: DataView;
};
/**
 * An SSZ type provides the following operations:
 * - Serialization from/to bytes to either a value or a tree
 * - Merkelization to compute the hashTreeRoot of both a value and a tree
 * - Proof creation from trees
 * - Create a View and a ViewDU instance from a tree
 * - Manipulate views
 */
export declare abstract class Type<V> {
    /**
     * If `true`, the type is basic.
     *
     * If `false`, the type is composite
     */
    abstract readonly isBasic: boolean;
    /** Tree depth to chunks or LeafNodes */
    abstract readonly depth: number;
    /** Maximum count of LeafNode chunks this type can have when merkleized */
    abstract readonly maxChunkCount: number;
    /**
     * The number of bytes of the serialized value.
     *
     * If `fixedSize === null`, the type has a variable serialized bytelength.
     */
    abstract readonly fixedSize: number | null;
    /** Minimum possible size of this type. Equals `this.fixedSize` if fixed size */
    abstract readonly minSize: number;
    /** Maximum possible size of this type. Equals `this.fixedSize` if fixed size */
    abstract readonly maxSize: number;
    /**
     * Human readable name
     *
     * @example
     * "List(Uint,4)"
     * "BeaconState"
     */
    abstract readonly typeName: string;
    /** INTERNAL METHOD: Return serialized size of a value */
    abstract value_serializedSize(value: V): number;
    /** INTERNAL METHOD: Serialize value to existing output ArrayBuffer views */
    abstract value_serializeToBytes(output: ByteViews, offset: number, value: V): number;
    /** INTERNAL METHOD: Deserialize value from a section of ArrayBuffer views */
    abstract value_deserializeFromBytes(data: ByteViews, start: number, end: number): V;
    /** INTERNAL METHOD: Return serialized size of a tree */
    abstract tree_serializedSize(node: Node): number;
    /** INTERNAL METHOD: Serialize tree to existing output ArrayBuffer views  */
    abstract tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    /** INTERNAL METHOD: Deserialize tree from a section of ArrayBuffer views */
    abstract tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    /** INTERNAL METHOD: Merkleize value to tree */
    value_toTree(value: V): Node;
    /** INTERNAL METHOD: Un-merkleize tree to value */
    tree_toValue(node: Node): V;
    /** New instance of a recursive zero'ed value of this type */
    abstract defaultValue(): V;
    /** Serialize a value to binary data */
    serialize(value: V): Uint8Array;
    /** Deserialize binary data to value */
    deserialize(uint8Array: Uint8Array): V;
    /**
     * Merkleize value and compute its hashTreeRoot.
     *
     * See spec for definition of hashTreeRoot:
     * https://github.com/ethereum/consensus-specs/blob/dev/ssz/simple-serialize.md#merkleization
     */
    abstract hashTreeRoot(value: V): Uint8Array;
    /** Parse JSON representation of a type to value */
    abstract fromJson(json: unknown): V;
    /** Convert value into its JSON representation */
    abstract toJson(value: V): unknown;
    /**
     * Returns a recursive clone of all mutable Types of a value, such that it can be safely mutated.
     *
     * Note: Immutable types and subtypes, such as `ByteVector`, return the original value.
     */
    abstract clone(value: V): V;
    /**
     * Returns true if values `a` and `b` are deeply equal by value
     */
    abstract equals(a: V, b: V): boolean;
}
//# sourceMappingURL=abstract.d.ts.map