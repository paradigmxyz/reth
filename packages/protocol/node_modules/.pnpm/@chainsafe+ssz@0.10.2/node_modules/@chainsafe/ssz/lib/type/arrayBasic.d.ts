import { Node } from "@chainsafe/persistent-merkle-tree";
import { Type, ValueOf, ByteViews } from "./abstract";
import { BasicType } from "./basic";
/**
 * SSZ Lists (variable-length arrays) include the length of the list in the tree
 * This length is always in the same index in the tree
 * ```
 *   1
 *  / \
 * 2   3 // <-here
 * ```
 */
export declare function getLengthFromRootNode(node: Node): number;
export declare function getChunksNodeFromRootNode(node: Node): Node;
export declare function addLengthNode(chunksNode: Node, length: number): Node;
export declare function setChunksNode(rootNode: Node, chunksNode: Node, newLength?: number): Node;
export declare type ArrayProps = {
    isList: true;
    limit: number;
} | {
    isList: false;
    length: number;
};
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
export declare function value_serializeToBytesArrayBasic<ElementType extends BasicType<unknown>>(elementType: ElementType, length: number, output: ByteViews, offset: number, value: ArrayLike<ValueOf<ElementType>>): number;
export declare function value_deserializeFromBytesArrayBasic<ElementType extends BasicType<unknown>>(elementType: ElementType, data: ByteViews, start: number, end: number, arrayProps: ArrayProps): ValueOf<ElementType>[];
/**
 * @param length In List length = value.length, Vector length = fixed value
 */
export declare function tree_serializeToBytesArrayBasic<ElementType extends BasicType<unknown>>(elementType: ElementType, length: number, depth: number, output: ByteViews, offset: number, node: Node): number;
export declare function tree_deserializeFromBytesArrayBasic<ElementType extends BasicType<unknown>>(elementType: ElementType, chunkDepth: number, data: ByteViews, start: number, end: number, arrayProps: ArrayProps): Node;
/**
 * @param length In List length = undefined, Vector length = fixed value
 */
export declare function value_fromJsonArray<ElementType extends Type<unknown>>(elementType: ElementType, json: unknown, arrayProps: ArrayProps): ValueOf<ElementType>[];
/**
 * @param length In List length = undefined, Vector length = fixed value
 */
export declare function value_toJsonArray<ElementType extends Type<unknown>>(elementType: ElementType, value: ValueOf<ElementType>[], arrayProps: ArrayProps): unknown[];
/**
 * Clone recursively an array of basic or composite types
 */
export declare function value_cloneArray<ElementType extends Type<unknown>>(elementType: ElementType, value: ValueOf<ElementType>[]): ValueOf<ElementType>[];
/**
 * Check recursively if a type is structuraly equal. Returns early
 */
export declare function value_equals<ElementType extends Type<unknown>>(elementType: ElementType, a: ValueOf<ElementType>[], b: ValueOf<ElementType>[]): boolean;
export declare function value_defaultValueArray<ElementType extends Type<unknown>>(elementType: ElementType, length: number): ValueOf<ElementType>[];
/**
 * @param checkNonDecimalLength Check that length is a multiple of element size.
 * Optional since it's not necessary in getOffsetsArrayComposite() fn.
 */
export declare function assertValidArrayLength(length: number, arrayProps: ArrayProps, checkNonDecimalLength?: boolean): void;
//# sourceMappingURL=arrayBasic.d.ts.map