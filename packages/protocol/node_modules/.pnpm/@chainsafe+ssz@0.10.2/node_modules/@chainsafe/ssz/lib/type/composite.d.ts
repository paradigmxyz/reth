import { Gindex, Node, Proof, Tree } from "@chainsafe/persistent-merkle-tree";
import { Type, ByteViews, JsonPath, JsonPathProp } from "./abstract";
export { ByteViews };
export declare const LENGTH_GINDEX: bigint;
/** View type of a CompositeType */
export declare type CompositeView<T extends CompositeType<unknown, unknown, unknown>> = T extends CompositeType<unknown, infer TV, unknown> ? TV : never;
/** ViewDU type of a CompositeType */
export declare type CompositeViewDU<T extends CompositeType<unknown, unknown, unknown>> = T extends CompositeType<unknown, unknown, infer TVDU> ? TVDU : never;
/** Any CompositeType without any generic arguments */
export declare type CompositeTypeAny = CompositeType<unknown, unknown, unknown>;
/**
 * Represents a composite type as defined in the spec:
 * https://github.com/ethereum/consensus-specs/blob/dev/ssz/simple-serialize.md#composite-types
 */
export declare abstract class CompositeType<V, TV, TVDU> extends Type<V> {
    /**
     * Caches `hashTreeRoot()` result for struct values.
     *
     * WARNING: Must only be used for immutable values. The cached root is never discarded
     */
    private readonly cachePermanentRootStruct?;
    readonly isBasic = false;
    /**
     * True if the merkleization of this type has a right node with metadata.
     * i.e. ListBasic, ListComposite, BitList, ByteList.
     */
    abstract readonly isList: boolean;
    /**
     * False if the TreeView of this type is immutable. Example:
     * - Any BasicType
     * - ByteVector, ByteList
     *
     * Required for ContainerNodeStruct to ensure no dangerous types are constructed.
     */
    abstract readonly isViewMutable: boolean;
    constructor(
    /**
     * Caches `hashTreeRoot()` result for struct values.
     *
     * WARNING: Must only be used for immutable values. The cached root is never discarded
     */
    cachePermanentRootStruct?: boolean | undefined);
    /** New instance of a recursive zero'ed value converted to Tree View */
    defaultView(): TV;
    /** New instance of a recursive zero'ed value converted to Deferred Update Tree View */
    defaultViewDU(): TVDU;
    /**
     * Returns a {@link TreeView}.
     *
     * A Tree View is a wrapper around a type and an SSZ Tree that contains:
     * - data merkleized
     * - a hook to its parent Tree to propagate changes upwards
     *
     * **View**
     * - Best for simple usage where performance is NOT important
     * - Applies changes immediately
     * - Has reference to parent tree
     * - Does NOT have caches for fast get / set ops
     *
     * **ViewDU**
     * - Best for complex usage where performance is important
     * - Defers changes to when commit is called
     * - Does NOT have a reference to the parent ViewDU
     * - Has caches for fast get / set ops
     */
    abstract getView(tree: Tree): TV;
    /**
     * Returns a {@link TreeViewDU} - Deferred Update Tree View.
     *
     * A Deferred Update Tree View is a wrapper around a type and
     * a SSZ Node that contains:
     * - data merkleized
     * - some arbitrary caches to speed up data manipulation required by the type
     *
     * **View**
     * - Best for simple usage where performance is NOT important
     * - Applies changes immediately
     * - Has reference to parent tree
     * - Does NOT have caches for fast get / set ops
     *
     * **ViewDU**
     * - Best for complex usage where performance is important
     * - Defers changes to when commit is called
     * - Does NOT have a reference to the parent ViewDU
     * - Has caches for fast get / set ops
     */
    abstract getViewDU(node: Node, cache?: unknown): TVDU;
    /** INTERNAL METHOD: Given a Tree View, returns a `Node` with all its updated data */
    abstract commitView(view: TV): Node;
    /** INTERNAL METHOD: Given a Deferred Update Tree View returns a `Node` with all its updated data */
    abstract commitViewDU(view: TVDU): Node;
    /** INTERNAL METHOD: Return the cache of a Deferred Update Tree View. May return `undefined` if this ViewDU has no cache */
    abstract cacheOfViewDU(view: TVDU): unknown;
    /**
     * Deserialize binary data to a Tree View.
     * @see {@link CompositeType.getView}
     */
    deserializeToView(data: Uint8Array): TV;
    /**
     * Deserialize binary data to a Deferred Update Tree View.
     * @see {@link CompositeType.getViewDU}
     */
    deserializeToViewDU(data: Uint8Array): TVDU;
    /**
     * Transform value to a View.
     * @see {@link CompositeType.getView}
     */
    toView(value: V): TV;
    /**
     * Transform value to a ViewDU.
     * @see {@link CompositeType.getViewDU}
     */
    toViewDU(value: V): TVDU;
    /**
     * Transform value to a View.
     * @see {@link CompositeType.getView}
     */
    toValueFromView(view: TV): V;
    /**
     * Transform value to a ViewDU.
     * @see {@link CompositeType.getViewDU}
     */
    toValueFromViewDU(view: TVDU): V;
    /**
     * Transform a ViewDU to a View.
     * @see {@link CompositeType.getView} and {@link CompositeType.getViewDU}
     */
    toViewFromViewDU(view: TVDU): TV;
    /**
     * Transform a View to a ViewDU.
     * @see {@link CompositeType.getView} and {@link CompositeType.getViewDU}
     */
    toViewDUFromView(view: TV): TVDU;
    hashTreeRoot(value: V): Uint8Array;
    protected getCachedPermanentRoot(value: V): Uint8Array | undefined;
    abstract readonly maxChunkCount: number;
    protected abstract getRoots(value: V): Uint8Array[];
    /**
     * Create a Tree View from a Proof. Verifies that the Proof is correct against `root`.
     * @see {@link CompositeType.getView}
     */
    createFromProof(proof: Proof, root?: Uint8Array): TV;
    /** INTERNAL METHOD: For view's API, create proof from a tree */
    tree_createProof(node: Node, jsonPaths: JsonPath[]): Proof;
    /** INTERNAL METHOD: For view's API, create proof from a tree */
    tree_createProofGindexes(node: Node, jsonPaths: JsonPath[]): Gindex[];
    /**
     * Navigate to a subtype & gindex using a path
     */
    getPathInfo(path: JsonPath): {
        gindex: Gindex;
        type: Type<unknown>;
    };
    /**
     * INTERNAL METHOD: post process `Ǹode` instance created from a proof and return either the same node,
     * and a new node representing the same data is a different `Node` instance. Currently used exclusively
     * by ContainerNodeStruct to convert `BranchNode` into `BranchNodeStruct`.
     */
    tree_fromProofNode(node: Node): {
        node: Node;
        done: boolean;
    };
    /**
     * Get leaf gindices
     *
     * Note: This is a recursively called method.
     * Subtypes recursively call this method until basic types / leaf data is hit.
     *
     * @param node Used for variable-length types.
     * @param root Used to anchor the returned gindices to a non-root gindex.
     * This is used to augment leaf gindices in recursively-called subtypes relative to the type.
     * @returns The gindices corresponding to leaf data.
     */
    abstract tree_getLeafGindices(rootGindex: Gindex, rootNode?: Node): Gindex[];
    /** Return the generalized index for the subtree. May return null if must not navigate below this type */
    abstract getPropertyGindex(property: JsonPathProp): Gindex | null;
    /** Return the property's subtype if the property exists */
    abstract getPropertyType(property: JsonPathProp): Type<unknown>;
    /** Return a leaf node index's property if the index is within bounds */
    abstract getIndexProperty(index: number): JsonPathProp | null;
}
export declare function isCompositeType(type: Type<unknown>): type is CompositeTypeAny;
//# sourceMappingURL=composite.d.ts.map