"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.UnionType = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const composite_1 = require("./composite");
const arrayBasic_1 = require("./arrayBasic");
const none_1 = require("./none");
const VALUE_GINDEX = BigInt(2);
const SELECTOR_GINDEX = BigInt(3);
/**
 * Union: union type containing one of the given subtypes
 * - Notation: Union[type_0, type_1, ...], e.g. union[None, uint64, uint32]
 */
class UnionType extends composite_1.CompositeType {
    constructor(types, opts) {
        super();
        this.types = types;
        this.depth = 1;
        this.maxChunkCount = 1;
        this.fixedSize = null;
        this.isList = true;
        this.isViewMutable = true;
        if (types.length >= 128) {
            throw Error("Must have less than 128 types");
        }
        if (types.length === 0) {
            throw Error("Must have at least 1 type option");
        }
        if (types[0] instanceof none_1.NoneType && types.length < 2) {
            throw Error("Must have at least 2 type options if the first is None");
        }
        for (let i = 1; i < types.length; i++) {
            if (types[i] instanceof none_1.NoneType) {
                throw Error("None may only be the first option");
            }
        }
        this.typeName = opts?.typeName ?? `Union[${types.map((t) => t.typeName).join(",")}]`;
        const minLens = [];
        const maxLens = [];
        for (const _type of types) {
            minLens.push(_type.minSize);
            maxLens.push(_type.maxSize);
        }
        this.minSize = 1 + Math.min(...minLens);
        this.maxSize = 1 + Math.max(...maxLens);
        this.maxSelector = this.types.length - 1;
    }
    static named(types, opts) {
        return new (named_1.namedClass(UnionType, opts.typeName))(types, opts);
    }
    defaultValue() {
        return {
            selector: 0,
            value: this.types[0].defaultValue(),
        };
    }
    getView(tree) {
        return this.tree_toValue(tree.rootNode);
    }
    getViewDU(node) {
        return this.tree_toValue(node);
    }
    cacheOfViewDU() {
        return;
    }
    commitView(view) {
        return this.value_toTree(view);
    }
    commitViewDU(view) {
        return this.value_toTree(view);
    }
    value_serializedSize(value) {
        return 1 + this.types[value.selector].value_serializedSize(value.value);
    }
    value_serializeToBytes(output, offset, value) {
        output.uint8Array[offset] = value.selector;
        return this.types[value.selector].value_serializeToBytes(output, offset + 1, value.value);
    }
    value_deserializeFromBytes(data, start, end) {
        const selector = data.uint8Array[start];
        if (selector > this.maxSelector) {
            throw Error(`Invalid selector ${selector}`);
        }
        return {
            selector,
            value: this.types[selector].value_deserializeFromBytes(data, start + 1, end),
        };
    }
    tree_serializedSize(node) {
        const selector = arrayBasic_1.getLengthFromRootNode(node);
        const valueNode = node.left;
        return 1 + this.types[selector].value_serializedSize(valueNode);
    }
    tree_serializeToBytes(output, offset, node) {
        const selector = arrayBasic_1.getLengthFromRootNode(node);
        const valueNode = node.left;
        output.uint8Array[offset] = selector;
        return this.types[selector].tree_serializeToBytes(output, offset + 1, valueNode);
    }
    tree_deserializeFromBytes(data, start, end) {
        const selector = data.uint8Array[start];
        if (selector > this.maxSelector) {
            throw Error(`Invalid selector ${selector}`);
        }
        const valueNode = this.types[selector].tree_deserializeFromBytes(data, start + 1, end);
        return arrayBasic_1.addLengthNode(valueNode, selector);
    }
    // Merkleization
    hashTreeRoot(value) {
        return merkleize_1.mixInLength(super.hashTreeRoot(value), value.selector);
    }
    getRoots(value) {
        const valueRoot = this.types[value.selector].hashTreeRoot(value.value);
        return [valueRoot];
    }
    // Proofs
    getPropertyGindex(prop) {
        switch (prop) {
            case "value":
                return VALUE_GINDEX;
            case "selector":
                return SELECTOR_GINDEX;
            default:
                throw new Error(`Invalid Union type property ${prop}`);
        }
    }
    getPropertyType() {
        // a Union has multiple types
        throw new Error("Not applicable for Union type");
    }
    getIndexProperty(index) {
        if (index === 0)
            return "value";
        if (index === 1)
            return "selector";
        throw Error("Union index of out bounds");
    }
    tree_getLeafGindices(rootGindex, rootNode) {
        if (!rootNode) {
            throw Error("rootNode required");
        }
        const gindices = [persistent_merkle_tree_1.concatGindices([rootGindex, SELECTOR_GINDEX])];
        const selector = arrayBasic_1.getLengthFromRootNode(rootNode);
        const type = this.types[selector];
        const extendedFieldGindex = persistent_merkle_tree_1.concatGindices([rootGindex, VALUE_GINDEX]);
        if (composite_1.isCompositeType(type)) {
            gindices.push(...type.tree_getLeafGindices(extendedFieldGindex, persistent_merkle_tree_1.getNode(rootNode, VALUE_GINDEX)));
        }
        else {
            gindices.push(extendedFieldGindex);
        }
        return gindices;
    }
    // JSON
    fromJson(json) {
        if (typeof json !== "object") {
            throw new Error("JSON must be of type object");
        }
        const union = json;
        if (typeof union.selector !== "number") {
            throw new Error("Invalid JSON Union selector must be number");
        }
        const type = this.types[union.selector];
        if (!type) {
            throw new Error("Invalid JSON Union selector out of range");
        }
        return {
            selector: union.selector,
            value: type.toJson(union.value),
        };
    }
    toJson(value) {
        return {
            selector: value.selector,
            value: this.types[value.selector].toJson(value.value),
        };
    }
    clone(value) {
        return {
            selector: value.selector,
            value: this.types[value.selector].clone(value.value),
        };
    }
    equals(a, b) {
        if (a.selector !== b.selector) {
            return false;
        }
        return this.types[a.selector].equals(a.value, b.value);
    }
}
exports.UnionType = UnionType;
//# sourceMappingURL=union.js.map