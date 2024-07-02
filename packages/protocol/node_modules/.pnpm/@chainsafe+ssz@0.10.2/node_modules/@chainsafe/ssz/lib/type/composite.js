"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.isCompositeType = exports.CompositeType = exports.LENGTH_GINDEX = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const byteArray_1 = require("../util/byteArray");
const merkleize_1 = require("../util/merkleize");
const treePostProcessFromProofNode_1 = require("../util/proof/treePostProcessFromProofNode");
const abstract_1 = require("./abstract");
exports.LENGTH_GINDEX = BigInt(3);
/** Dedicated property to cache hashTreeRoot of immutable CompositeType values */
const symbolCachedPermanentRoot = Symbol("ssz_cached_permanent_root");
/* eslint-disable @typescript-eslint/member-ordering  */
/**
 * Represents a composite type as defined in the spec:
 * https://github.com/ethereum/consensus-specs/blob/dev/ssz/simple-serialize.md#composite-types
 */
class CompositeType extends abstract_1.Type {
    constructor(
    /**
     * Caches `hashTreeRoot()` result for struct values.
     *
     * WARNING: Must only be used for immutable values. The cached root is never discarded
     */
    cachePermanentRootStruct) {
        super();
        this.cachePermanentRootStruct = cachePermanentRootStruct;
        this.isBasic = false;
    }
    /** New instance of a recursive zero'ed value converted to Tree View */
    defaultView() {
        return this.toView(this.defaultValue());
    }
    /** New instance of a recursive zero'ed value converted to Deferred Update Tree View */
    defaultViewDU() {
        return this.toViewDU(this.defaultValue());
    }
    /**
     * Deserialize binary data to a Tree View.
     * @see {@link CompositeType.getView}
     */
    deserializeToView(data) {
        const dataView = new DataView(data.buffer, data.byteOffset, data.byteLength);
        const node = this.tree_deserializeFromBytes({ uint8Array: data, dataView }, 0, data.length);
        return this.getView(new persistent_merkle_tree_1.Tree(node));
    }
    /**
     * Deserialize binary data to a Deferred Update Tree View.
     * @see {@link CompositeType.getViewDU}
     */
    deserializeToViewDU(data) {
        const dataView = new DataView(data.buffer, data.byteOffset, data.byteLength);
        const node = this.tree_deserializeFromBytes({ uint8Array: data, dataView }, 0, data.length);
        return this.getViewDU(node);
    }
    /**
     * Transform value to a View.
     * @see {@link CompositeType.getView}
     */
    toView(value) {
        const node = this.value_toTree(value);
        return this.getView(new persistent_merkle_tree_1.Tree(node));
    }
    /**
     * Transform value to a ViewDU.
     * @see {@link CompositeType.getViewDU}
     */
    toViewDU(value) {
        const node = this.value_toTree(value);
        return this.getViewDU(node);
    }
    /**
     * Transform value to a View.
     * @see {@link CompositeType.getView}
     */
    toValueFromView(view) {
        const node = this.commitView(view);
        return this.tree_toValue(node);
    }
    /**
     * Transform value to a ViewDU.
     * @see {@link CompositeType.getViewDU}
     */
    toValueFromViewDU(view) {
        const node = this.commitViewDU(view);
        return this.tree_toValue(node);
    }
    /**
     * Transform a ViewDU to a View.
     * @see {@link CompositeType.getView} and {@link CompositeType.getViewDU}
     */
    toViewFromViewDU(view) {
        const node = this.commitViewDU(view);
        return this.getView(new persistent_merkle_tree_1.Tree(node));
    }
    /**
     * Transform a View to a ViewDU.
     * @see {@link CompositeType.getView} and {@link CompositeType.getViewDU}
     */
    toViewDUFromView(view) {
        const node = this.commitView(view);
        return this.getViewDU(node);
    }
    // Merkleize API
    hashTreeRoot(value) {
        // Return cached mutable root if any
        if (this.cachePermanentRootStruct) {
            const cachedRoot = value[symbolCachedPermanentRoot];
            if (cachedRoot) {
                return cachedRoot;
            }
        }
        const root = merkleize_1.merkleize(this.getRoots(value), this.maxChunkCount);
        if (this.cachePermanentRootStruct) {
            value[symbolCachedPermanentRoot] = root;
        }
        return root;
    }
    // For debugging and testing this feature
    getCachedPermanentRoot(value) {
        return value[symbolCachedPermanentRoot];
    }
    // Proofs API
    /**
     * Create a Tree View from a Proof. Verifies that the Proof is correct against `root`.
     * @see {@link CompositeType.getView}
     */
    createFromProof(proof, root) {
        const rootNodeFromProof = persistent_merkle_tree_1.Tree.createFromProof(proof).rootNode;
        const rootNode = treePostProcessFromProofNode_1.treePostProcessFromProofNode(rootNodeFromProof, this);
        if (root !== undefined && !byteArray_1.byteArrayEquals(rootNode.root, root)) {
            throw new Error("Proof does not match trusted root");
        }
        return this.getView(new persistent_merkle_tree_1.Tree(rootNode));
    }
    /** INTERNAL METHOD: For view's API, create proof from a tree */
    tree_createProof(node, jsonPaths) {
        const gindexes = this.tree_createProofGindexes(node, jsonPaths);
        return persistent_merkle_tree_1.createProof(node, {
            type: persistent_merkle_tree_1.ProofType.treeOffset,
            gindices: gindexes,
        });
    }
    /** INTERNAL METHOD: For view's API, create proof from a tree */
    tree_createProofGindexes(node, jsonPaths) {
        const gindexes = [];
        for (const jsonPath of jsonPaths) {
            const { type, gindex } = this.getPathInfo(jsonPath);
            if (!isCompositeType(type)) {
                gindexes.push(gindex);
            }
            else {
                // if the path subtype is composite, include the gindices of all the leaves
                const leafGindexes = type.tree_getLeafGindices(gindex, type.fixedSize === null ? persistent_merkle_tree_1.getNode(node, gindex) : undefined);
                for (const gindex of leafGindexes) {
                    gindexes.push(gindex);
                }
            }
        }
        return gindexes;
    }
    /**
     * Navigate to a subtype & gindex using a path
     */
    getPathInfo(path) {
        const gindices = [];
        let type = this;
        for (const prop of path) {
            if (type.isBasic) {
                throw new Error("Invalid path: cannot navigate beyond a basic type");
            }
            const gindex = type.getPropertyGindex(prop);
            // else stop navigating
            if (gindex !== null) {
                gindices.push(gindex);
                type = type.getPropertyType(prop);
            }
        }
        return {
            type,
            gindex: persistent_merkle_tree_1.concatGindices(gindices),
        };
    }
    /**
     * INTERNAL METHOD: post process `Ǹode` instance created from a proof and return either the same node,
     * and a new node representing the same data is a different `Node` instance. Currently used exclusively
     * by ContainerNodeStruct to convert `BranchNode` into `BranchNodeStruct`.
     */
    tree_fromProofNode(node) {
        return { node, done: false };
    }
}
exports.CompositeType = CompositeType;
function isCompositeType(type) {
    return !type.isBasic;
}
exports.isCompositeType = isCompositeType;
//# sourceMappingURL=composite.js.map