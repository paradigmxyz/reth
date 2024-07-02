import { Node } from "@chainsafe/persistent-merkle-tree";
import { Type } from "../type/abstract";
import { BasicType } from "../type/basic";
import { CompositeType } from "../type/composite";
import { ContainerTypeGeneric } from "../view/container";
import { TreeViewDU } from "./abstract";
export declare type FieldsViewDU<Fields extends Record<string, Type<unknown>>> = {
    [K in keyof Fields]: Fields[K] extends CompositeType<unknown, unknown, infer TVDU> ? TVDU : Fields[K] extends BasicType<infer V> ? V : never;
};
export declare type ContainerTreeViewDUType<Fields extends Record<string, Type<unknown>>> = FieldsViewDU<Fields> & TreeViewDU<ContainerTypeGeneric<Fields>>;
export declare type ContainerTreeViewDUTypeConstructor<Fields extends Record<string, Type<unknown>>> = {
    new (type: ContainerTypeGeneric<Fields>, node: Node, cache?: unknown): ContainerTreeViewDUType<Fields>;
};
export declare function getContainerTreeViewDUClass<Fields extends Record<string, Type<unknown>>>(type: ContainerTypeGeneric<Fields>): ContainerTreeViewDUTypeConstructor<Fields>;
//# sourceMappingURL=container.d.ts.map