import { Node, Tree, Gindex } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { Type } from "./abstract";
import { CompositeType, ByteViews } from "./composite";
import { getContainerTreeViewClass } from "../view/container";
import { ValueOfFields, FieldEntry, ContainerTreeViewType, ContainerTreeViewTypeConstructor } from "../view/container";
import { getContainerTreeViewDUClass, ContainerTreeViewDUType, ContainerTreeViewDUTypeConstructor } from "../viewDU/container";
declare type BytesRange = {
    start: number;
    end: number;
};
export declare type ContainerOptions<Fields extends Record<string, unknown>> = {
    typeName?: string;
    jsonCase?: KeyCase;
    casingMap?: CasingMap<Fields>;
    cachePermanentRootStruct?: boolean;
    getContainerTreeViewClass?: typeof getContainerTreeViewClass;
    getContainerTreeViewDUClass?: typeof getContainerTreeViewDUClass;
};
declare type KeyCase = "eth2" | "snake" | "constant" | "camel" | "header" | "pascal";
declare type CasingMap<Fields extends Record<string, unknown>> = Partial<{
    [K in keyof Fields]: string;
}>;
/**
 * Container: ordered heterogeneous collection of values
 * - Notation: Custom name per instance
 */
export declare class ContainerType<Fields extends Record<string, Type<unknown>>> extends CompositeType<ValueOfFields<Fields>, ContainerTreeViewType<Fields>, ContainerTreeViewDUType<Fields>> {
    readonly fields: Fields;
    readonly opts?: ContainerOptions<Fields> | undefined;
    readonly typeName: string;
    readonly depth: number;
    readonly maxChunkCount: number;
    readonly fixedSize: number | null;
    readonly minSize: number;
    readonly maxSize: number;
    readonly isList = false;
    readonly isViewMutable = true;
    readonly fieldsEntries: FieldEntry<Fields>[];
    protected readonly fieldsGindex: Record<keyof Fields, Gindex>;
    protected readonly jsonKeyToFieldName: Record<string, keyof Fields>;
    protected readonly isFixedLen: boolean[];
    protected readonly fieldRangesFixedLen: BytesRange[];
    /** Offsets position relative to start of serialized Container. Length may not equal field count. */
    protected readonly variableOffsetsPosition: number[];
    /** End of fixed section of serialized Container */
    protected readonly fixedEnd: number;
    /** Cached TreeView constuctor with custom prototype for this Type's properties */
    protected readonly TreeView: ContainerTreeViewTypeConstructor<Fields>;
    protected readonly TreeViewDU: ContainerTreeViewDUTypeConstructor<Fields>;
    constructor(fields: Fields, opts?: ContainerOptions<Fields> | undefined);
    static named<Fields extends Record<string, Type<unknown>>>(fields: Fields, opts: Require<ContainerOptions<Fields>, "typeName">): ContainerType<Fields>;
    defaultValue(): ValueOfFields<Fields>;
    getView(tree: Tree): ContainerTreeViewType<Fields>;
    getViewDU(node: Node, cache?: unknown): ContainerTreeViewDUType<Fields>;
    cacheOfViewDU(view: ContainerTreeViewDUType<Fields>): unknown;
    commitView(view: ContainerTreeViewType<Fields>): Node;
    commitViewDU(view: ContainerTreeViewDUType<Fields>): Node;
    value_serializedSize(value: ValueOfFields<Fields>): number;
    value_serializeToBytes(output: ByteViews, offset: number, value: ValueOfFields<Fields>): number;
    value_deserializeFromBytes(data: ByteViews, start: number, end: number): ValueOfFields<Fields>;
    tree_serializedSize(node: Node): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    protected getRoots(struct: ValueOfFields<Fields>): Uint8Array[];
    getPropertyGindex(prop: string): Gindex | null;
    getPropertyType(prop: string): Type<unknown>;
    getIndexProperty(index: number): string | null;
    tree_getLeafGindices(rootGindex: Gindex, rootNode?: Node): Gindex[];
    fromJson(json: unknown): ValueOfFields<Fields>;
    toJson(value: ValueOfFields<Fields>): Record<string, unknown>;
    clone(value: ValueOfFields<Fields>): ValueOfFields<Fields>;
    equals(a: ValueOfFields<Fields>, b: ValueOfFields<Fields>): boolean;
    /**
     * Deserializer helper: Returns the bytes ranges of all fields, both variable and fixed size.
     * Fields may not be contiguous in the serialized bytes, so the returned ranges are [start, end].
     * - For fixed size fields re-uses the pre-computed values this.fieldRangesFixedLen
     * - For variable size fields does a first pass over the fixed section to read offsets
     */
    private getFieldRanges;
}
/**
 * Compute the JSON key for each fieldName. There will exist a single JSON representation for each type.
 * To transform JSON payloads to a casing that is different from the type's defined use external tooling.
 */
export declare function precomputeJsonKey<Fields extends Record<string, Type<unknown>>>(fieldName: keyof Fields, casingMap?: CasingMap<Fields>, jsonCase?: KeyCase): string;
/**
 * Render field typeNames for a detailed typeName of this Container
 */
export declare function renderContainerTypeName<Fields extends Record<string, Type<unknown>>>(fields: Fields, prefix?: string): string;
export {};
//# sourceMappingURL=container.d.ts.map