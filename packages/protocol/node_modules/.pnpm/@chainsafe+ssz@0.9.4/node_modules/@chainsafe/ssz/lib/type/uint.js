"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.UintBigintType = exports.UintNumberType = exports.uintBigintByteLens = exports.uintNumberByteLens = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const named_1 = require("../util/named");
const basic_1 = require("./basic");
/* eslint-disable @typescript-eslint/member-ordering */
const MAX_SAFE_INTEGER_BN = BigInt(Number.MAX_SAFE_INTEGER);
const BIGINT_2_POW_64 = BigInt(2) ** BigInt(64);
const BIGINT_2_POW_128 = BigInt(2) ** BigInt(128);
const BIGINT_2_POW_192 = BigInt(2) ** BigInt(192);
// const BIGINT_64_MAX = BigInt("0xffffffffffffffff");
const NUMBER_2_POW_32 = 2 ** 32;
const NUMBER_32_MAX = 0xffffffff;
exports.uintNumberByteLens = [1, 2, 4, 8];
exports.uintBigintByteLens = [1, 2, 4, 8, 16, 32];
/**
 * Uint: N-bit unsigned integer (where N in [8, 16, 32, 64, 128, 256])
 * - Notation: uintN
 *
 * UintNumber is represented as the Javascript primitive value 'Number'.
 *
 * The Number type is a double-precision 64-bit binary format IEEE 754 value (numbers between -(2^53 − 1) and
 * 2^53 − 1). It also has the symbolic value: +Infinity.
 *
 * As of 2021 performance of 'Number' is extremely faster than 'BigInt'. Some values are spec'ed as Uint64 but
 * practically they will never exceed 53 bits, such as any unit time or simple counters. This type is an optimization
 * for these cases, as UintNumber64 can represent any value between 0 and 2^53−1 as well as the max value 2^64-1.
 */
class UintNumberType extends basic_1.BasicType {
    constructor(byteLength, opts) {
        super();
        this.byteLength = byteLength;
        if (byteLength > 8) {
            throw Error("UintNumber byteLength limit is 8");
        }
        if (Math.log2(byteLength) % 1 !== 0) {
            throw Error("byteLength must be a power of 2");
        }
        this.typeName = opts?.typeName ?? `uint${byteLength * 8}`;
        if (opts?.clipInfinity)
            this.typeName += "Inf";
        if (opts?.setBitwiseOR)
            this.typeName += "OR";
        this.itemsPerChunk = 32 / this.byteLength;
        this.fixedSize = byteLength;
        this.minSize = byteLength;
        this.maxSize = byteLength;
        this.maxDecimalStr = (BigInt(2) ** BigInt(this.byteLength * 8) - BigInt(1)).toString(10);
        this.clipInfinity = opts?.clipInfinity === true;
        this.setBitwiseOR = opts?.setBitwiseOR === true;
    }
    static named(byteLength, opts) {
        return new (named_1.namedClass(UintNumberType, opts.typeName))(byteLength, opts);
    }
    defaultValue() {
        return 0;
    }
    // Serialization + deserialization
    value_serializeToBytes({ dataView }, offset, value) {
        switch (this.byteLength) {
            case 1:
                dataView.setInt8(offset, value);
                break;
            case 2:
                dataView.setUint16(offset, value, true);
                break;
            case 4:
                dataView.setUint32(offset, value, true);
                break;
            case 8:
                if (value === Infinity) {
                    // TODO: Benchmark if it's faster to set BIGINT_64_MAX once
                    dataView.setUint32(offset, 0xffffffff);
                    dataView.setUint32(offset + 4, 0xffffffff);
                }
                else {
                    dataView.setUint32(offset, value & 0xffffffff, true);
                    dataView.setUint32(offset + 4, (value / NUMBER_2_POW_32) & 0xffffffff, true);
                }
                break;
        }
        return offset + this.byteLength;
    }
    value_deserializeFromBytes({ dataView }, start, end) {
        this.assertValidSize(end - start);
        switch (this.byteLength) {
            case 1:
                return dataView.getUint8(start);
            case 2:
                return dataView.getUint16(start, true);
            case 4:
                return dataView.getUint32(start, true);
            case 8: {
                const a = dataView.getUint32(start, true);
                const b = dataView.getUint32(start + 4, true);
                if (b === NUMBER_32_MAX && a === NUMBER_32_MAX && this.clipInfinity) {
                    return Infinity;
                }
                else {
                    return b * NUMBER_2_POW_32 + a;
                }
            }
        }
    }
    tree_serializeToBytes(output, offset, node) {
        const value = node.getUint(this.byteLength, 0, this.clipInfinity);
        this.value_serializeToBytes(output, offset, value);
        return offset + this.byteLength;
    }
    tree_deserializeFromBytes(data, start, end) {
        this.assertValidSize(end - start);
        const value = this.value_deserializeFromBytes(data, start, end);
        const node = persistent_merkle_tree_1.LeafNode.fromZero();
        node.setUint(this.byteLength, 0, value, this.clipInfinity);
        return node;
    }
    // Fast Tree access
    tree_getFromNode(leafNode) {
        return leafNode.getUint(this.byteLength, 0, this.clipInfinity);
    }
    tree_setToNode(leafNode, value) {
        this.tree_setToPackedNode(leafNode, 0, value);
    }
    tree_getFromPackedNode(leafNode, index) {
        const offsetBytes = this.byteLength * (index % this.itemsPerChunk);
        return leafNode.getUint(this.byteLength, offsetBytes, this.clipInfinity);
    }
    tree_setToPackedNode(leafNode, index, value) {
        const offsetBytes = this.byteLength * (index % this.itemsPerChunk);
        // TODO: Benchmark the cost of this if, and consider using a different class
        if (this.setBitwiseOR) {
            leafNode.bitwiseOrUint(this.byteLength, offsetBytes, value);
        }
        else {
            leafNode.setUint(this.byteLength, offsetBytes, value, this.clipInfinity);
        }
    }
    // JSON
    fromJson(json) {
        if (typeof json === "number") {
            return json;
        }
        else if (typeof json === "string") {
            if (this.clipInfinity && json === this.maxDecimalStr) {
                // Allow to handle max possible number
                return Infinity;
            }
            else {
                const num = parseInt(json, 10);
                if (isNaN(num)) {
                    throw Error("JSON invalid number isNaN");
                }
                else if (num > Number.MAX_SAFE_INTEGER) {
                    // Throw to prevent decimal precision errors downstream
                    throw Error("JSON invalid number > MAX_SAFE_INTEGER");
                }
                else {
                    return num;
                }
            }
        }
        else if (typeof json === "bigint") {
            if (json > MAX_SAFE_INTEGER_BN) {
                // Throw to prevent decimal precision errors downstream
                throw Error("JSON invalid number > MAX_SAFE_INTEGER_BN");
            }
            else {
                return Number(json);
            }
        }
        else {
            throw Error(`JSON invalid type ${typeof json} expected number`);
        }
    }
    toJson(value) {
        if (value === Infinity) {
            return this.maxDecimalStr;
        }
        else {
            return value.toString(10);
        }
    }
}
exports.UintNumberType = UintNumberType;
/**
 * Uint: N-bit unsigned integer (where N in [8, 16, 32, 64, 128, 256])
 * - Notation: uintN
 *
 * UintBigint is represented as the Javascript primitive value 'BigInt'.
 *
 * The BigInt type is a numeric primitive in JavaScript that can represent integers with arbitrary precision.
 * With BigInts, you can safely store and operate on large integers even beyond the safe integer limit for Numbers.
 *
 * As of 2021 performance of 'Number' is extremely faster than 'BigInt'. For Uint values under 53 bits use UintNumber.
 * For other values that may exceed 53 bits, use UintBigint.
 */
class UintBigintType extends basic_1.BasicType {
    constructor(byteLength, opts) {
        super();
        this.byteLength = byteLength;
        if (byteLength > 32) {
            throw Error("UintBigint byteLength limit is 32");
        }
        if (Math.log2(byteLength) % 1 !== 0) {
            throw Error("byteLength must be a power of 2");
        }
        this.typeName = opts?.typeName ?? `uintBigint${byteLength * 8}`;
        this.byteLength = byteLength;
        this.itemsPerChunk = 32 / this.byteLength;
        this.fixedSize = byteLength;
        this.minSize = byteLength;
        this.maxSize = byteLength;
    }
    static named(byteLength, opts) {
        return new (named_1.namedClass(UintBigintType, opts.typeName))(byteLength, opts);
    }
    defaultValue() {
        return BigInt(0);
    }
    // Serialization + deserialization
    value_serializeToBytes({ dataView }, offset, value) {
        switch (this.byteLength) {
            case 1:
                dataView.setInt8(offset, Number(value));
                break;
            case 2:
                dataView.setUint16(offset, Number(value), true);
                break;
            case 4:
                dataView.setUint32(offset, Number(value), true);
                break;
            case 8:
                dataView.setBigUint64(offset, value, true);
                break;
            default: {
                for (let i = 0; i < this.byteLength; i += 8) {
                    if (i > 0)
                        value = value / BIGINT_2_POW_64;
                    const lo = BigInt.asUintN(64, value);
                    dataView.setBigUint64(offset + i, lo, true);
                }
            }
        }
        return offset + this.byteLength;
    }
    value_deserializeFromBytes({ dataView }, start, end) {
        const size = end - start;
        if (size !== this.byteLength) {
            throw Error(`Invalid size ${size} expected ${this.byteLength}`);
        }
        // Note: pre-assigning the right function at the constructor to avoid this switch is not faster
        switch (this.byteLength) {
            case 1:
                return BigInt(dataView.getUint8(start));
            case 2:
                return BigInt(dataView.getUint16(start, true));
            case 4:
                return BigInt(dataView.getUint32(start, true));
            case 8:
                return dataView.getBigUint64(start, true);
            case 16: {
                const a = dataView.getBigUint64(start, true);
                const b = dataView.getBigUint64(start + 8, true);
                return b * BIGINT_2_POW_64 + a;
            }
            case 32: {
                const a = dataView.getBigUint64(start, true);
                const b = dataView.getBigUint64(start + 8, true);
                const c = dataView.getBigUint64(start + 16, true);
                const d = dataView.getBigUint64(start + 24, true);
                return d * BIGINT_2_POW_192 + c * BIGINT_2_POW_128 + b * BIGINT_2_POW_64 + a;
            }
        }
    }
    tree_serializeToBytes(output, offset, node) {
        const value = node.getUintBigint(this.byteLength, 0);
        this.value_serializeToBytes(output, offset, value);
        return offset + this.byteLength;
    }
    tree_deserializeFromBytes(data, start, end) {
        const size = end - start;
        if (size !== this.byteLength) {
            throw Error(`Invalid size ${size} expected ${this.byteLength}`);
        }
        const value = this.value_deserializeFromBytes(data, start, end);
        const node = persistent_merkle_tree_1.LeafNode.fromZero();
        node.setUintBigint(this.byteLength, 0, value);
        return node;
    }
    // Fast Tree access
    tree_getFromNode(leafNode) {
        return leafNode.getUintBigint(this.byteLength, 0);
    }
    /** Mutates node to set value */
    tree_setToNode(leafNode, value) {
        this.tree_setToPackedNode(leafNode, 0, value);
    }
    /** EXAMPLE of `tree_getFromNode` */
    tree_getFromPackedNode(leafNode, index) {
        const offsetBytes = this.byteLength * (index % this.itemsPerChunk);
        return leafNode.getUintBigint(this.byteLength, offsetBytes);
    }
    /** Mutates node to set value */
    tree_setToPackedNode(leafNode, index, value) {
        const offsetBytes = this.byteLength * (index % this.itemsPerChunk);
        // TODO: Not-optimized, copy pasted from UintNumberType
        leafNode.setUintBigint(this.byteLength, offsetBytes, value);
    }
    // JSON
    fromJson(json) {
        if (typeof json === "bigint") {
            return json;
        }
        else if (typeof json === "string" || typeof json === "number") {
            return BigInt(json);
        }
        else {
            throw Error(`JSON invalid type ${typeof json} expected bigint`);
        }
    }
    toJson(value) {
        return value.toString(10);
    }
}
exports.UintBigintType = UintBigintType;
//# sourceMappingURL=uint.js.map