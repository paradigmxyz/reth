import { Node } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { ByteViews } from "./composite";
import { ByteArrayType, ByteArray } from "./byteArray";
export interface ByteListOptions {
    typeName?: string;
}
/**
 * ByteList: Immutable alias of List[byte, N]
 * - Notation: `ByteList[N]`
 * - Value: `Uint8Array`
 * - View: `Uint8Array`
 * - ViewDU: `Uint8Array`
 *
 * ByteList is an immutable value which is represented by a Uint8Array for memory efficiency and performance.
 * Note: Consumers of this type MUST never mutate the `Uint8Array` representation of a ByteList.
 *
 * For a `ByteListType` with mutability, use `ListBasicType(byteType)`
 */
export declare class ByteListType extends ByteArrayType {
    readonly limitBytes: number;
    readonly typeName: string;
    readonly depth: number;
    readonly chunkDepth: number;
    readonly fixedSize: null;
    readonly minSize: number;
    readonly maxSize: number;
    readonly maxChunkCount: number;
    readonly isList = true;
    constructor(limitBytes: number, opts?: ByteListOptions);
    static named(limitBits: number, opts: Require<ByteListOptions, "typeName">): ByteListType;
    value_serializedSize(value: Uint8Array): number;
    tree_serializedSize(node: Node): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getByteLen(node?: Node): number;
    hashTreeRoot(value: ByteArray): Uint8Array;
    protected assertValidSize(size: number): void;
}
//# sourceMappingURL=byteList.d.ts.map