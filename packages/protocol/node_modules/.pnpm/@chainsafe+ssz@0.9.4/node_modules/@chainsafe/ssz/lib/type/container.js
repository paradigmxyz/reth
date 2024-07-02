"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.renderContainerTypeName = exports.precomputeJsonKey = exports.ContainerType = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const case_1 = __importDefault(require("case"));
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const composite_1 = require("./composite");
const container_1 = require("../view/container");
const container_2 = require("../viewDU/container");
/**
 * Container: ordered heterogeneous collection of values
 * - Notation: Custom name per instance
 */
class ContainerType extends composite_1.CompositeType {
    constructor(fields, opts) {
        super(opts?.cachePermanentRootStruct);
        this.fields = fields;
        this.opts = opts;
        this.isList = false;
        this.isViewMutable = true;
        // Render detailed typeName. Consumers should overwrite since it can get long
        this.typeName = opts?.typeName ?? renderContainerTypeName(fields);
        this.maxChunkCount = Object.keys(fields).length;
        this.depth = merkleize_1.maxChunksToDepth(this.maxChunkCount);
        // Precalculated data for faster serdes
        this.fieldsEntries = [];
        for (const fieldName of Object.keys(fields)) {
            this.fieldsEntries.push({
                fieldName,
                fieldType: this.fields[fieldName],
                jsonKey: precomputeJsonKey(fieldName, opts?.casingMap, opts?.jsonCase),
                gindex: persistent_merkle_tree_1.toGindex(this.depth, BigInt(this.fieldsEntries.length)),
            });
        }
        if (this.fieldsEntries.length === 0) {
            throw Error("Container must have > 0 fields");
        }
        // Precalculate for Proofs API
        this.fieldsGindex = {};
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            this.fieldsGindex[this.fieldsEntries[i].fieldName] = persistent_merkle_tree_1.toGindex(this.depth, BigInt(i));
        }
        // To resolve JSON paths in fieldName notation and jsonKey notation
        this.jsonKeyToFieldName = {};
        for (const { fieldName, jsonKey } of this.fieldsEntries) {
            this.jsonKeyToFieldName[jsonKey] = fieldName;
        }
        const { minLen, maxLen, fixedSize } = precomputeSizes(fields);
        this.minSize = minLen;
        this.maxSize = maxLen;
        this.fixedSize = fixedSize;
        const { isFixedLen, fieldRangesFixedLen, variableOffsetsPosition, fixedEnd } = precomputeSerdesData(fields);
        this.isFixedLen = isFixedLen;
        this.fieldRangesFixedLen = fieldRangesFixedLen;
        this.variableOffsetsPosition = variableOffsetsPosition;
        this.fixedEnd = fixedEnd;
        // TODO: This options are necessary for ContainerNodeStruct to override this.
        // Refactor this constructor to allow customization without pollutin the options
        this.TreeView = opts?.getContainerTreeViewClass?.(this) ?? container_1.getContainerTreeViewClass(this);
        this.TreeViewDU = opts?.getContainerTreeViewDUClass?.(this) ?? container_2.getContainerTreeViewDUClass(this);
    }
    static named(fields, opts) {
        return new (named_1.namedClass(ContainerType, opts.typeName))(fields, opts);
    }
    defaultValue() {
        const value = {};
        for (const { fieldName, fieldType } of this.fieldsEntries) {
            value[fieldName] = fieldType.defaultValue();
        }
        return value;
    }
    getView(tree) {
        return new this.TreeView(this, tree);
    }
    getViewDU(node, cache) {
        return new this.TreeViewDU(this, node, cache);
    }
    cacheOfViewDU(view) {
        return view.cache;
    }
    commitView(view) {
        return view.node;
    }
    commitViewDU(view) {
        view.commit();
        return view.node;
    }
    // Serialization + deserialization
    // -------------------------------
    // Containers can mix fixed length and variable length data.
    //
    // Fixed part                         Variable part
    // [field1 offset][field2 data       ][field1 data               ]
    // [0x000000c]    [0xaabbaabbaabbaabb][0xffffffffffffffffffffffff]
    value_serializedSize(value) {
        let totalSize = 0;
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldName, fieldType } = this.fieldsEntries[i];
            // Offset (4 bytes) + size
            totalSize +=
                fieldType.fixedSize === null ? 4 + fieldType.value_serializedSize(value[fieldName]) : fieldType.fixedSize;
        }
        return totalSize;
    }
    value_serializeToBytes(output, offset, value) {
        let fixedIndex = offset;
        let variableIndex = offset + this.fixedEnd;
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldName, fieldType } = this.fieldsEntries[i];
            if (fieldType.fixedSize === null) {
                // write offset
                output.dataView.setUint32(fixedIndex, variableIndex - offset, true);
                fixedIndex += 4;
                // write serialized element to variable section
                variableIndex = fieldType.value_serializeToBytes(output, variableIndex, value[fieldName]);
            }
            else {
                fixedIndex = fieldType.value_serializeToBytes(output, fixedIndex, value[fieldName]);
            }
        }
        return variableIndex;
    }
    value_deserializeFromBytes(data, start, end) {
        const fieldRanges = this.getFieldRanges(data.dataView, start, end);
        const value = {};
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldName, fieldType } = this.fieldsEntries[i];
            const fieldRange = fieldRanges[i];
            value[fieldName] = fieldType.value_deserializeFromBytes(data, start + fieldRange.start, start + fieldRange.end);
        }
        return value;
    }
    tree_serializedSize(node) {
        let totalSize = 0;
        const nodes = persistent_merkle_tree_1.getNodesAtDepth(node, this.depth, 0, this.fieldsEntries.length);
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldType } = this.fieldsEntries[i];
            const node = nodes[i];
            // Offset (4 bytes) + size
            totalSize += fieldType.fixedSize === null ? 4 + fieldType.tree_serializedSize(node) : fieldType.fixedSize;
        }
        return totalSize;
    }
    tree_serializeToBytes(output, offset, node) {
        let fixedIndex = offset;
        let variableIndex = offset + this.fixedEnd;
        const nodes = persistent_merkle_tree_1.getNodesAtDepth(node, this.depth, 0, this.fieldsEntries.length);
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldType } = this.fieldsEntries[i];
            const node = nodes[i];
            if (fieldType.fixedSize === null) {
                // write offset
                output.dataView.setUint32(fixedIndex, variableIndex - offset, true);
                fixedIndex += 4;
                // write serialized element to variable section
                variableIndex = fieldType.tree_serializeToBytes(output, variableIndex, node);
            }
            else {
                fixedIndex = fieldType.tree_serializeToBytes(output, fixedIndex, node);
            }
        }
        return variableIndex;
    }
    tree_deserializeFromBytes(data, start, end) {
        const fieldRanges = this.getFieldRanges(data.dataView, start, end);
        const nodes = new Array(this.fieldsEntries.length);
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldType } = this.fieldsEntries[i];
            const fieldRange = fieldRanges[i];
            nodes[i] = fieldType.tree_deserializeFromBytes(data, start + fieldRange.start, start + fieldRange.end);
        }
        return persistent_merkle_tree_1.subtreeFillToContents(nodes, this.depth);
    }
    // Merkleization
    getRoots(struct) {
        const roots = new Array(this.fieldsEntries.length);
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldName, fieldType } = this.fieldsEntries[i];
            roots[i] = fieldType.hashTreeRoot(struct[fieldName]);
        }
        return roots;
    }
    // Proofs
    // getPropertyGindex
    // getPropertyType
    // tree_getLeafGindices
    getPropertyGindex(prop) {
        const gindex = this.fieldsGindex[prop] ?? this.fieldsGindex[this.jsonKeyToFieldName[prop]];
        if (gindex === undefined)
            throw Error(`Unknown container property ${prop}`);
        return gindex;
    }
    getPropertyType(prop) {
        const type = this.fields[prop] ?? this.fields[this.jsonKeyToFieldName[prop]];
        if (type === undefined)
            throw Error(`Unknown container property ${prop}`);
        return type;
    }
    getIndexProperty(index) {
        if (index >= this.fieldsEntries.length) {
            return null;
        }
        return this.fieldsEntries[index].fieldName;
    }
    tree_getLeafGindices(rootGindex, rootNode) {
        const gindices = [];
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldName, fieldType } = this.fieldsEntries[i];
            const fieldGindex = this.fieldsGindex[fieldName];
            const fieldGindexFromRoot = persistent_merkle_tree_1.concatGindices([rootGindex, fieldGindex]);
            if (fieldType.isBasic) {
                gindices.push(fieldGindexFromRoot);
            }
            else {
                const compositeType = fieldType;
                if (fieldType.fixedSize === null) {
                    if (!rootNode) {
                        throw new Error("variable type requires tree argument to get leaves");
                    }
                    gindices.push(...compositeType.tree_getLeafGindices(fieldGindexFromRoot, persistent_merkle_tree_1.getNode(rootNode, fieldGindex)));
                }
                else {
                    gindices.push(...compositeType.tree_getLeafGindices(fieldGindexFromRoot));
                }
            }
        }
        return gindices;
    }
    // JSON
    fromJson(json) {
        if (typeof json !== "object") {
            throw Error("JSON must be of type object");
        }
        if (json === null) {
            throw Error("JSON must not be null");
        }
        const value = {};
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldName, fieldType, jsonKey } = this.fieldsEntries[i];
            const jsonValue = json[jsonKey];
            if (jsonValue === undefined) {
                throw Error(`JSON expected key ${jsonKey} is undefined`);
            }
            value[fieldName] = fieldType.fromJson(jsonValue);
        }
        return value;
    }
    toJson(value) {
        const json = {};
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldName, fieldType, jsonKey } = this.fieldsEntries[i];
            json[jsonKey] = fieldType.toJson(value[fieldName]);
        }
        return json;
    }
    clone(value) {
        const newValue = {};
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldName, fieldType } = this.fieldsEntries[i];
            newValue[fieldName] = fieldType.clone(value[fieldName]);
        }
        return newValue;
    }
    equals(a, b) {
        for (let i = 0; i < this.fieldsEntries.length; i++) {
            const { fieldName, fieldType } = this.fieldsEntries[i];
            if (!fieldType.equals(a[fieldName], b[fieldName])) {
                return false;
            }
        }
        return true;
    }
    /**
     * Deserializer helper: Returns the bytes ranges of all fields, both variable and fixed size.
     * Fields may not be contiguous in the serialized bytes, so the returned ranges are [start, end].
     * - For fixed size fields re-uses the pre-computed values this.fieldRangesFixedLen
     * - For variable size fields does a first pass over the fixed section to read offsets
     */
    getFieldRanges(data, start, end) {
        if (this.variableOffsetsPosition.length === 0) {
            // Validate fixed length container
            const size = end - start;
            if (size !== this.fixedEnd) {
                throw Error(`${this.typeName} size ${size} not equal fixed size ${this.fixedEnd}`);
            }
            return this.fieldRangesFixedLen;
        }
        // Read offsets in one pass
        const offsets = readVariableOffsets(data, start, end, this.fixedEnd, this.variableOffsetsPosition);
        offsets.push(end - start); // The offsets are relative to the start
        // Merge fieldRangesFixedLen + offsets in one array
        let variableIdx = 0;
        let fixedIdx = 0;
        const fieldRanges = new Array(this.isFixedLen.length);
        for (let i = 0; i < this.isFixedLen.length; i++) {
            if (this.isFixedLen[i]) {
                // push from fixLen ranges ++
                fieldRanges[i] = this.fieldRangesFixedLen[fixedIdx++];
            }
            else {
                // push from varLen ranges ++
                fieldRanges[i] = { start: offsets[variableIdx], end: offsets[variableIdx + 1] };
                variableIdx++;
            }
        }
        return fieldRanges;
    }
}
exports.ContainerType = ContainerType;
/**
 * Returns the byte ranges of all variable size fields.
 */
function readVariableOffsets(data, start, end, fixedEnd, variableOffsetsPosition) {
    // Since variable-sized values can be interspersed with fixed-sized values, we precalculate
    // the offset indices so we can more easily deserialize the fields in once pass first we get the fixed sizes
    // Note: `fixedSizes[i] = null` if that field has variable length
    const size = end - start;
    // with the fixed sizes, we can read the offsets, and store for our single pass
    const offsets = new Array(variableOffsetsPosition.length);
    for (let i = 0; i < variableOffsetsPosition.length; i++) {
        const offset = data.getUint32(start + variableOffsetsPosition[i], true);
        // Validate offsets. If the list is empty the offset points to the end of the buffer, offset == size
        if (offset > size) {
            throw new Error(`Offset out of bounds ${offset} > ${size}`);
        }
        if (i === 0) {
            if (offset !== fixedEnd) {
                throw new Error(`First offset must equal to fixedEnd ${offset} != ${fixedEnd}`);
            }
        }
        else {
            if (offset < offsets[i - 1]) {
                throw new Error(`Offsets must be increasing ${offset} < ${offsets[i - 1]}`);
            }
        }
        offsets[i] = offset;
    }
    return offsets;
}
/**
 * Precompute fixed and variable offsets position for faster deserialization.
 * @returns Does a single pass over all fields and returns:
 * - isFixedLen: If field index [i] is fixed length
 * - fieldRangesFixedLen: For fields with fixed length, their range of bytes
 * - variableOffsetsPosition: Position of the 4 bytes offset for variable size fields
 * - fixedEnd: End of the fixed size range
 * -
 */
function precomputeSerdesData(fields) {
    const isFixedLen = [];
    const fieldRangesFixedLen = [];
    const variableOffsetsPosition = [];
    let pointerFixed = 0;
    for (const fieldType of Object.values(fields)) {
        isFixedLen.push(fieldType.fixedSize !== null);
        if (fieldType.fixedSize === null) {
            // Variable length
            variableOffsetsPosition.push(pointerFixed);
            pointerFixed += 4;
        }
        else {
            fieldRangesFixedLen.push({ start: pointerFixed, end: pointerFixed + fieldType.fixedSize });
            pointerFixed += fieldType.fixedSize;
        }
    }
    return {
        isFixedLen,
        fieldRangesFixedLen,
        variableOffsetsPosition,
        fixedEnd: pointerFixed,
    };
}
/**
 * Precompute sizes of the Container doing one pass over fields
 */
function precomputeSizes(fields) {
    let minLen = 0;
    let maxLen = 0;
    let fixedSize = 0;
    for (const fieldType of Object.values(fields)) {
        minLen += fieldType.minSize;
        maxLen += fieldType.maxSize;
        if (fieldType.fixedSize === null) {
            // +4 for the offset
            minLen += 4;
            maxLen += 4;
            fixedSize = null;
        }
        else if (fixedSize !== null) {
            fixedSize += fieldType.fixedSize;
        }
    }
    return { minLen, maxLen, fixedSize };
}
/**
 * Compute the JSON key for each fieldName. There will exist a single JSON representation for each type.
 * To transform JSON payloads to a casing that is different from the type's defined use external tooling.
 */
function precomputeJsonKey(fieldName, casingMap, jsonCase) {
    if (casingMap) {
        const keyFromCaseMap = casingMap[fieldName];
        if (keyFromCaseMap === undefined) {
            throw Error(`casingMap[${fieldName}] not defined`);
        }
        return keyFromCaseMap;
    }
    else if (jsonCase) {
        if (jsonCase === "eth2") {
            const snake = case_1.default.snake(fieldName);
            return snake.replace(/(\d)$/, "_$1");
        }
        else {
            return case_1.default[jsonCase](fieldName);
        }
    }
    else {
        return fieldName;
    }
}
exports.precomputeJsonKey = precomputeJsonKey;
/**
 * Render field typeNames for a detailed typeName of this Container
 */
function renderContainerTypeName(fields, prefix = "Container") {
    const fieldNames = Object.keys(fields);
    const fieldTypeNames = fieldNames.map((fieldName) => `${fieldName}: ${fields[fieldName].typeName}`).join(", ");
    return `${prefix}({${fieldTypeNames}})`;
}
exports.renderContainerTypeName = renderContainerTypeName;
//# sourceMappingURL=container.js.map