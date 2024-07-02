import { Gindex, Tree } from "@chainsafe/persistent-merkle-tree";
import { Type, ValueOf } from "../type/abstract";
import { BasicType } from "../type/basic";
import { CompositeType } from "../type/composite";
import { TreeView } from "./abstract";
export declare type FieldEntry<Fields extends Record<string, Type<unknown>>> = {
    fieldName: keyof Fields;
    fieldType: Fields[keyof Fields];
    jsonKey: string;
    gindex: Gindex;
};
/** Expected API of this View's type. This interface allows to break a recursive dependency between types and views */
export declare type ContainerTypeGeneric<Fields extends Record<string, Type<unknown>>> = CompositeType<ValueOfFields<Fields>, ContainerTreeViewType<Fields>, unknown> & {
    readonly fields: Fields;
    readonly fieldsEntries: FieldEntry<Fields>[];
};
export declare type ValueOfFields<Fields extends Record<string, Type<unknown>>> = {
    [K in keyof Fields]: ValueOf<Fields[K]>;
};
export declare type FieldsView<Fields extends Record<string, Type<unknown>>> = {
    [K in keyof Fields]: Fields[K] extends CompositeType<unknown, infer TV, unknown> ? TV : Fields[K] extends BasicType<infer V> ? V : never;
};
export declare type ContainerTreeViewType<Fields extends Record<string, Type<unknown>>> = FieldsView<Fields> & TreeView<ContainerTypeGeneric<Fields>>;
export declare type ContainerTreeViewTypeConstructor<Fields extends Record<string, Type<unknown>>> = {
    new (type: ContainerTypeGeneric<Fields>, tree: Tree): ContainerTreeViewType<Fields>;
};
export declare function getContainerTreeViewClass<Fields extends Record<string, Type<unknown>>>(type: ContainerTypeGeneric<Fields>): ContainerTreeViewTypeConstructor<Fields>;
//# sourceMappingURL=container.d.ts.map