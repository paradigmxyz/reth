import { Node } from "@chainsafe/persistent-merkle-tree";
import { ValueOf, Type } from "./abstract";
import { CompositeType } from "./composite";
/**
 * Array: ordered homogeneous collection
 */
export declare abstract class ArrayType<ElementType extends Type<unknown>, TV, TVDU> extends CompositeType<ValueOf<ElementType>[], TV, TVDU> {
    readonly elementType: ElementType;
    abstract readonly itemsPerChunk: number;
    protected abstract readonly defaultLen: number;
    constructor(elementType: ElementType);
    defaultValue(): ValueOf<ElementType>[];
    abstract tree_getLength(node: Node): number;
    getPropertyType(): Type<unknown>;
    getPropertyGindex(prop: string | number): bigint;
    getIndexProperty(index: number): string | number;
    tree_getLeafGindices(rootGindex: bigint, rootNode?: Node): bigint[];
    fromJson(json: unknown): ValueOf<ElementType>[];
    toJson(value: ValueOf<ElementType>[]): unknown;
    clone(value: ValueOf<ElementType>[]): ValueOf<ElementType>[];
    equals(a: ValueOf<ElementType>[], b: ValueOf<ElementType>[]): boolean;
}
//# sourceMappingURL=array.d.ts.map