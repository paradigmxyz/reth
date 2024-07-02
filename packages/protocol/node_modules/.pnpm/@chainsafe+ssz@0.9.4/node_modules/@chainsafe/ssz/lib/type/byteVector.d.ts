import { Node } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { ByteViews } from "./composite";
import { ByteArrayType } from "./byteArray";
export declare type ByteVector = Uint8Array;
export interface ByteVectorOptions {
    typeName?: string;
}
/**
 * ByteVector: Immutable alias of Vector[byte, N]
 * - Notation: `ByteVector[N]`
 * - Value: `Uint8Array`
 * - View: `Uint8Array`
 * - ViewDU: `Uint8Array`
 *
 * ByteVector is an immutable value which is represented by a Uint8Array for memory efficiency and performance.
 * Note: Consumers of this type MUST never mutate the `Uint8Array` representation of a ByteVector.
 *
 * For a `ByteVectorType` with mutability, use `VectorBasicType(byteType)`
 */
export declare class ByteVectorType extends ByteArrayType {
    readonly lengthBytes: number;
    readonly typeName: string;
    readonly depth: number;
    readonly chunkDepth: number;
    readonly fixedSize: number;
    readonly minSize: number;
    readonly maxSize: number;
    readonly maxChunkCount: number;
    readonly isList = false;
    constructor(lengthBytes: number, opts?: ByteVectorOptions);
    static named(limitBits: number, opts: Require<ByteVectorOptions, "typeName">): ByteVectorType;
    value_serializedSize(): number;
    tree_serializedSize(): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getByteLen(): number;
    protected assertValidSize(size: number): void;
}
//# sourceMappingURL=byteVector.d.ts.map