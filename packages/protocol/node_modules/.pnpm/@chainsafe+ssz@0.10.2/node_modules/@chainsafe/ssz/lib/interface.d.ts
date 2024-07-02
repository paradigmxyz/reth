/**
 * These interfaces are consistent across all backings.
 * As long as these interfaces are respected, the backing can be abstracted entirely.
 */
export interface ArrayLike<T> {
    [n: number]: T;
    readonly length: number;
    [Symbol.iterator](): Iterator<T>;
}
export declare type Vector<T> = ArrayLike<T>;
export interface List<T> extends ArrayLike<T> {
    push(...values: T[]): number;
    pop(): T | undefined;
}
export declare type Container<T extends Record<string, unknown>> = T;
export declare type ByteVector = Vector<number>;
export declare type ByteList = List<number>;
export declare type BitVector = Vector<boolean>;
export declare type BitList = List<boolean>;
export interface ObjectLike {
    [fieldName: string]: any;
}
export interface Union<T> {
    readonly selector: number;
    value: T;
}
export declare type CompositeValue = Record<string, any> | ArrayLike<unknown> | Union<unknown> | Record<string, never>;
/**
 * The Json interface is used for json-serializable input
 */
export declare type Json = string | number | boolean | null | {
    [property: string]: Json;
} | Json[];
//# sourceMappingURL=interface.d.ts.map