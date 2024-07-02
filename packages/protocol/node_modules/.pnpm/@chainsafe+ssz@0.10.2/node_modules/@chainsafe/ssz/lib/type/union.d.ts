import { Node, Tree } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { Type, ByteViews } from "./abstract";
import { CompositeType } from "./composite";
declare type Union<T> = {
    readonly selector: number;
    value: T;
};
declare type ValueOfTypes<Types extends Type<unknown>[]> = Types extends Type<infer T>[] ? Union<T> : never;
export declare type UnionOpts = {
    typeName?: string;
};
/**
 * Union: union type containing one of the given subtypes
 * - Notation: Union[type_0, type_1, ...], e.g. union[None, uint64, uint32]
 */
export declare class UnionType<Types extends Type<unknown>[]> extends CompositeType<ValueOfTypes<Types>, ValueOfTypes<Types>, ValueOfTypes<Types>> {
    readonly types: Types;
    readonly typeName: string;
    readonly depth = 1;
    readonly maxChunkCount = 1;
    readonly fixedSize: null;
    readonly minSize: number;
    readonly maxSize: number;
    readonly isList = true;
    readonly isViewMutable = true;
    protected readonly maxSelector: number;
    constructor(types: Types, opts?: UnionOpts);
    static named<Types extends Type<unknown>[]>(types: Types, opts: Require<UnionOpts, "typeName">): UnionType<Types>;
    defaultValue(): ValueOfTypes<Types>;
    getView(tree: Tree): ValueOfTypes<Types>;
    getViewDU(node: Node): ValueOfTypes<Types>;
    cacheOfViewDU(): unknown;
    commitView(view: ValueOfTypes<Types>): Node;
    commitViewDU(view: ValueOfTypes<Types>): Node;
    value_serializedSize(value: ValueOfTypes<Types>): number;
    value_serializeToBytes(output: ByteViews, offset: number, value: ValueOfTypes<Types>): number;
    value_deserializeFromBytes(data: ByteViews, start: number, end: number): ValueOfTypes<Types>;
    tree_serializedSize(node: Node): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    hashTreeRoot(value: ValueOfTypes<Types>): Uint8Array;
    protected getRoots(value: ValueOfTypes<Types>): Uint8Array[];
    getPropertyGindex(prop: string): bigint;
    getPropertyType(): never;
    getIndexProperty(index: number): string | number;
    tree_getLeafGindices(rootGindex: bigint, rootNode?: Node): bigint[];
    fromJson(json: unknown): ValueOfTypes<Types>;
    toJson(value: ValueOfTypes<Types>): Record<string, unknown>;
    clone(value: ValueOfTypes<Types>): ValueOfTypes<Types>;
    equals(a: ValueOfTypes<Types>, b: ValueOfTypes<Types>): boolean;
}
export {};
//# sourceMappingURL=union.d.ts.map