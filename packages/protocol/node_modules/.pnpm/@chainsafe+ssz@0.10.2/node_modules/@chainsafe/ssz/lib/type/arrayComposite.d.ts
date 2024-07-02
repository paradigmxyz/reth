import { Node } from "@chainsafe/persistent-merkle-tree";
import { ValueOf, ByteViews } from "./abstract";
import { CompositeType } from "./composite";
export declare type ArrayProps = {
    isList: true;
    limit: number;
} | {
    isList: false;
    length: number;
};
export declare function minSizeArrayComposite<ElementType extends CompositeType<unknown, unknown, unknown>>(elementType: ElementType, minCount: number): number;
export declare function maxSizeArrayComposite<ElementType extends CompositeType<unknown, unknown, unknown>>(elementType: ElementType, maxCount: number): number;
export declare function value_serializedSizeArrayComposite<ElementType extends CompositeType<unknown, unknown, unknown>>(elementType: ElementType, length: number, value: ValueOf<ElementType>[]): number;
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
export declare function value_serializeToBytesArrayComposite<ElementType extends CompositeType<unknown, unknown, unknown>>(elementType: ElementType, length: number, output: ByteViews, offset: number, value: ValueOf<ElementType>[]): number;
export declare function value_deserializeFromBytesArrayComposite<ElementType extends CompositeType<ValueOf<ElementType>, unknown, unknown>>(elementType: ElementType, data: ByteViews, start: number, end: number, arrayProps: ArrayProps): ValueOf<ElementType>[];
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
export declare function tree_serializedSizeArrayComposite<ElementType extends CompositeType<unknown, unknown, unknown>>(elementType: ElementType, length: number, depth: number, node: Node): number;
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
export declare function tree_serializeToBytesArrayComposite<ElementType extends CompositeType<unknown, unknown, unknown>>(elementType: ElementType, length: number, depth: number, node: Node, output: ByteViews, offset: number): number;
export declare function tree_deserializeFromBytesArrayComposite<ElementType extends CompositeType<unknown, unknown, unknown>>(elementType: ElementType, chunkDepth: number, data: ByteViews, start: number, end: number, arrayProps: ArrayProps): Node;
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
export declare function value_getRootsArrayComposite<ElementType extends CompositeType<unknown, unknown, unknown>>(elementType: ElementType, length: number, value: ValueOf<ElementType>[]): Uint8Array[];
//# sourceMappingURL=arrayComposite.d.ts.map