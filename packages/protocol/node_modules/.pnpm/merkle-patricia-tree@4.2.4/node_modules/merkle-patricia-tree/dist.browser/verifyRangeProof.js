"use strict";
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __generator = (this && this.__generator) || function (thisArg, body) {
    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g;
    return g = { next: verb(0), "throw": verb(1), "return": verb(2) }, typeof Symbol === "function" && (g[Symbol.iterator] = function() { return this; }), g;
    function verb(n) { return function (v) { return step([n, v]); }; }
    function step(op) {
        if (f) throw new TypeError("Generator is already executing.");
        while (_) try {
            if (f = 1, y && (t = op[0] & 2 ? y["return"] : op[0] ? y["throw"] || ((t = y["return"]) && t.call(y), 0) : y.next) && !(t = t.call(y, op[1])).done) return t;
            if (y = 0, t) op = [op[0] & 2, t.value];
            switch (op[0]) {
                case 0: case 1: t = op; break;
                case 4: _.label++; return { value: op[1], done: false };
                case 5: _.label++; y = op[1]; op = [0]; continue;
                case 7: op = _.ops.pop(); _.trys.pop(); continue;
                default:
                    if (!(t = _.trys, t = t.length > 0 && t[t.length - 1]) && (op[0] === 6 || op[0] === 2)) { _ = 0; continue; }
                    if (op[0] === 3 && (!t || (op[1] > t[0] && op[1] < t[3]))) { _.label = op[1]; break; }
                    if (op[0] === 6 && _.label < t[1]) { _.label = t[1]; t = op; break; }
                    if (t && _.label < t[2]) { _.label = t[2]; _.ops.push(op); break; }
                    if (t[2]) _.ops.pop();
                    _.trys.pop(); continue;
            }
            op = body.call(thisArg, _);
        } catch (e) { op = [6, e]; y = 0; } finally { f = t = 0; }
        if (op[0] & 5) throw op[1]; return { value: op[0] ? op[1] : void 0, done: true };
    }
};
var __read = (this && this.__read) || function (o, n) {
    var m = typeof Symbol === "function" && o[Symbol.iterator];
    if (!m) return o;
    var i = m.call(o), r, ar = [], e;
    try {
        while ((n === void 0 || n-- > 0) && !(r = i.next()).done) ar.push(r.value);
    }
    catch (error) { e = { error: error }; }
    finally {
        try {
            if (r && !r.done && (m = i["return"])) m.call(i);
        }
        finally { if (e) throw e.error; }
    }
    return ar;
};
var __spreadArray = (this && this.__spreadArray) || function (to, from, pack) {
    if (pack || arguments.length === 2) for (var i = 0, l = from.length, ar; i < l; i++) {
        if (ar || !(i in from)) {
            if (!ar) ar = Array.prototype.slice.call(from, 0, i);
            ar[i] = from[i];
        }
    }
    return to.concat(ar || Array.prototype.slice.call(from));
};
var __values = (this && this.__values) || function(o) {
    var s = typeof Symbol === "function" && Symbol.iterator, m = s && o[s], i = 0;
    if (m) return m.call(o);
    if (o && typeof o.length === "number") return {
        next: function () {
            if (o && i >= o.length) o = void 0;
            return { value: o && o[i++], done: !o };
        }
    };
    throw new TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.verifyRangeProof = void 0;
var nibbles_1 = require("./util/nibbles");
var baseTrie_1 = require("./baseTrie");
var trieNode_1 = require("./trieNode");
// reference: https://github.com/ethereum/go-ethereum/blob/20356e57b119b4e70ce47665a71964434e15200d/trie/proof.go
/**
 * unset will remove all nodes to the left or right of the target key(decided by `removeLeft`).
 * @param trie - trie object.
 * @param parent - parent node, it can be `null`.
 * @param child - child node.
 * @param key - target nibbles.
 * @param pos - key position.
 * @param removeLeft - remove all nodes to the left or right of the target key.
 * @param stack - a stack of modified nodes.
 * @returns The end position of key.
 */
function unset(trie, parent, child, key, pos, removeLeft, stack) {
    return __awaiter(this, void 0, void 0, function () {
        var i, i, next, _child, _a, _child;
        return __generator(this, function (_b) {
            switch (_b.label) {
                case 0:
                    if (!(child instanceof trieNode_1.BranchNode)) return [3 /*break*/, 4];
                    /**
                     * This node is a branch node,
                     * remove all branches on the left or right
                     */
                    if (removeLeft) {
                        for (i = 0; i < key[pos]; i++) {
                            child.setBranch(i, null);
                        }
                    }
                    else {
                        for (i = key[pos] + 1; i < 16; i++) {
                            child.setBranch(i, null);
                        }
                    }
                    // record this node on the stack
                    stack.push(child);
                    next = child.getBranch(key[pos]);
                    _a = next;
                    if (!_a) return [3 /*break*/, 2];
                    return [4 /*yield*/, trie.lookupNode(next)];
                case 1:
                    _a = (_b.sent());
                    _b.label = 2;
                case 2:
                    _child = _a;
                    return [4 /*yield*/, unset(trie, child, _child, key, pos + 1, removeLeft, stack)];
                case 3: return [2 /*return*/, _b.sent()];
                case 4:
                    if (!(child instanceof trieNode_1.ExtensionNode || child instanceof trieNode_1.LeafNode)) return [3 /*break*/, 9];
                    /**
                     * This node is an extension node or lead node,
                     * if node._nibbles is less or greater than the target key,
                     * remove self from parent
                     */
                    if (key.length - pos < child.keyLength ||
                        (0, nibbles_1.nibblesCompare)(child._nibbles, key.slice(pos, pos + child.keyLength)) !== 0) {
                        if (removeLeft) {
                            if ((0, nibbles_1.nibblesCompare)(child._nibbles, key.slice(pos)) < 0) {
                                ;
                                parent.setBranch(key[pos - 1], null);
                            }
                        }
                        else {
                            if ((0, nibbles_1.nibblesCompare)(child._nibbles, key.slice(pos)) > 0) {
                                ;
                                parent.setBranch(key[pos - 1], null);
                            }
                        }
                        return [2 /*return*/, pos - 1];
                    }
                    if (!(child instanceof trieNode_1.LeafNode)) return [3 /*break*/, 5];
                    // This node is a leaf node, directly remove it from parent
                    ;
                    parent.setBranch(key[pos - 1], null);
                    return [2 /*return*/, pos - 1];
                case 5: return [4 /*yield*/, trie.lookupNode(child.value)];
                case 6:
                    _child = _b.sent();
                    if (_child && _child instanceof trieNode_1.LeafNode) {
                        // The child of this node is leaf node, remove it from parent too
                        ;
                        parent.setBranch(key[pos - 1], null);
                        return [2 /*return*/, pos - 1];
                    }
                    // record this node on the stack
                    stack.push(child);
                    return [4 /*yield*/, unset(trie, child, _child, key, pos + child.keyLength, removeLeft, stack)];
                case 7: 
                // continue to the next node
                return [2 /*return*/, _b.sent()];
                case 8: return [3 /*break*/, 10];
                case 9:
                    if (child === null) {
                        return [2 /*return*/, pos - 1];
                    }
                    else {
                        throw new Error('invalid node');
                    }
                    _b.label = 10;
                case 10: return [2 /*return*/];
            }
        });
    });
}
/**
 * unsetInternal will remove all nodes between `left` and `right` (including `left` and `right`)
 * @param trie - trie object.
 * @param left - left nibbles.
 * @param right - right nibbles.
 * @returns Is it an empty trie.
 */
function unsetInternal(trie, left, right) {
    return __awaiter(this, void 0, void 0, function () {
        var pos, parent, node, shortForkLeft, shortForkRight, stack, leftNode, rightNode, abort, i, saveStack, removeSelfFromParentAndSaveStack, child, endPos, child, endPos, i, _stack, next, child, _a, endPos, _stack, next, child, _b, endPos;
        var _this = this;
        return __generator(this, function (_c) {
            switch (_c.label) {
                case 0:
                    pos = 0;
                    parent = null;
                    return [4 /*yield*/, trie.lookupNode(trie.root)];
                case 1:
                    node = _c.sent();
                    stack = [];
                    _c.label = 2;
                case 2:
                    if (!true) return [3 /*break*/, 8];
                    if (!(node instanceof trieNode_1.ExtensionNode || node instanceof trieNode_1.LeafNode)) return [3 /*break*/, 4];
                    // record this node on the stack
                    stack.push(node);
                    if (left.length - pos < node.keyLength) {
                        shortForkLeft = (0, nibbles_1.nibblesCompare)(left.slice(pos), node._nibbles);
                    }
                    else {
                        shortForkLeft = (0, nibbles_1.nibblesCompare)(left.slice(pos, pos + node.keyLength), node._nibbles);
                    }
                    if (right.length - pos < node.keyLength) {
                        shortForkRight = (0, nibbles_1.nibblesCompare)(right.slice(pos), node._nibbles);
                    }
                    else {
                        shortForkRight = (0, nibbles_1.nibblesCompare)(right.slice(pos, pos + node.keyLength), node._nibbles);
                    }
                    // If one of `left` and `right` is not equal to node._nibbles, it means we found the fork point
                    if (shortForkLeft !== 0 || shortForkRight !== 0) {
                        return [3 /*break*/, 8];
                    }
                    if (node instanceof trieNode_1.LeafNode) {
                        // it shouldn't happen
                        throw new Error('invalid node');
                    }
                    // continue to the next node
                    parent = node;
                    pos += node.keyLength;
                    return [4 /*yield*/, trie.lookupNode(node.value)];
                case 3:
                    node = _c.sent();
                    return [3 /*break*/, 7];
                case 4:
                    if (!(node instanceof trieNode_1.BranchNode)) return [3 /*break*/, 6];
                    // record this node on the stack
                    stack.push(node);
                    leftNode = node.getBranch(left[pos]);
                    rightNode = node.getBranch(right[pos]);
                    // One of `left` and `right` is `null`, stop searching
                    if (leftNode === null || rightNode === null) {
                        return [3 /*break*/, 8];
                    }
                    // Stop searching if `left` and `right` are not equal
                    if (!(leftNode instanceof Buffer)) {
                        if (rightNode instanceof Buffer) {
                            return [3 /*break*/, 8];
                        }
                        if (leftNode.length !== rightNode.length) {
                            return [3 /*break*/, 8];
                        }
                        abort = false;
                        for (i = 0; i < leftNode.length; i++) {
                            if (leftNode[i].compare(rightNode[i]) !== 0) {
                                abort = true;
                                break;
                            }
                        }
                        if (abort) {
                            return [3 /*break*/, 8];
                        }
                    }
                    else {
                        if (!(rightNode instanceof Buffer)) {
                            return [3 /*break*/, 8];
                        }
                        if (leftNode.compare(rightNode) !== 0) {
                            return [3 /*break*/, 8];
                        }
                    }
                    // continue to the next node
                    parent = node;
                    return [4 /*yield*/, trie.lookupNode(leftNode)];
                case 5:
                    node = _c.sent();
                    pos += 1;
                    return [3 /*break*/, 7];
                case 6: throw new Error('invalid node');
                case 7: return [3 /*break*/, 2];
                case 8:
                    saveStack = function (key, stack) {
                        return trie._saveStack(key, stack, []);
                    };
                    if (!(node instanceof trieNode_1.ExtensionNode || node instanceof trieNode_1.LeafNode)) return [3 /*break*/, 27];
                    removeSelfFromParentAndSaveStack = function (key) { return __awaiter(_this, void 0, void 0, function () {
                        return __generator(this, function (_a) {
                            switch (_a.label) {
                                case 0:
                                    if (parent === null) {
                                        return [2 /*return*/, true];
                                    }
                                    stack.pop();
                                    parent.setBranch(key[pos - 1], null);
                                    return [4 /*yield*/, saveStack(key.slice(0, pos - 1), stack)];
                                case 1:
                                    _a.sent();
                                    return [2 /*return*/, false];
                            }
                        });
                    }); };
                    if (shortForkLeft === -1 && shortForkRight === -1) {
                        throw new Error('invalid range');
                    }
                    if (shortForkLeft === 1 && shortForkRight === 1) {
                        throw new Error('invalid range');
                    }
                    if (!(shortForkLeft !== 0 && shortForkRight !== 0)) return [3 /*break*/, 10];
                    return [4 /*yield*/, removeSelfFromParentAndSaveStack(left)];
                case 9: 
                // Unset the entire trie
                return [2 /*return*/, _c.sent()];
                case 10:
                    if (!(shortForkRight !== 0)) return [3 /*break*/, 18];
                    if (!(node instanceof trieNode_1.LeafNode)) return [3 /*break*/, 12];
                    return [4 /*yield*/, removeSelfFromParentAndSaveStack(left)];
                case 11: return [2 /*return*/, _c.sent()];
                case 12: return [4 /*yield*/, trie.lookupNode(node._value)];
                case 13:
                    child = _c.sent();
                    if (!(child && child instanceof trieNode_1.LeafNode)) return [3 /*break*/, 15];
                    return [4 /*yield*/, removeSelfFromParentAndSaveStack(left)];
                case 14: return [2 /*return*/, _c.sent()];
                case 15: return [4 /*yield*/, unset(trie, node, child, left.slice(pos), node.keyLength, false, stack)];
                case 16:
                    endPos = _c.sent();
                    return [4 /*yield*/, saveStack(left.slice(0, pos + endPos), stack)];
                case 17:
                    _c.sent();
                    return [2 /*return*/, false];
                case 18:
                    if (!(shortForkLeft !== 0)) return [3 /*break*/, 26];
                    if (!(node instanceof trieNode_1.LeafNode)) return [3 /*break*/, 20];
                    return [4 /*yield*/, removeSelfFromParentAndSaveStack(right)];
                case 19: return [2 /*return*/, _c.sent()];
                case 20: return [4 /*yield*/, trie.lookupNode(node._value)];
                case 21:
                    child = _c.sent();
                    if (!(child && child instanceof trieNode_1.LeafNode)) return [3 /*break*/, 23];
                    return [4 /*yield*/, removeSelfFromParentAndSaveStack(right)];
                case 22: return [2 /*return*/, _c.sent()];
                case 23: return [4 /*yield*/, unset(trie, node, child, right.slice(pos), node.keyLength, true, stack)];
                case 24:
                    endPos = _c.sent();
                    return [4 /*yield*/, saveStack(right.slice(0, pos + endPos), stack)];
                case 25:
                    _c.sent();
                    return [2 /*return*/, false];
                case 26: return [2 /*return*/, false];
                case 27:
                    if (!(node instanceof trieNode_1.BranchNode)) return [3 /*break*/, 36];
                    // Unset all internal nodes in the forkpoint
                    for (i = left[pos] + 1; i < right[pos]; i++) {
                        node.setBranch(i, null);
                    }
                    _stack = __spreadArray([], __read(stack), false);
                    next = node.getBranch(left[pos]);
                    _a = next;
                    if (!_a) return [3 /*break*/, 29];
                    return [4 /*yield*/, trie.lookupNode(next)];
                case 28:
                    _a = (_c.sent());
                    _c.label = 29;
                case 29:
                    child = _a;
                    return [4 /*yield*/, unset(trie, node, child, left.slice(pos), 1, false, _stack)];
                case 30:
                    endPos = _c.sent();
                    return [4 /*yield*/, saveStack(left.slice(0, pos + endPos), _stack)];
                case 31:
                    _c.sent();
                    _stack = __spreadArray([], __read(stack), false);
                    next = node.getBranch(right[pos]);
                    _b = next;
                    if (!_b) return [3 /*break*/, 33];
                    return [4 /*yield*/, trie.lookupNode(next)];
                case 32:
                    _b = (_c.sent());
                    _c.label = 33;
                case 33:
                    child = _b;
                    return [4 /*yield*/, unset(trie, node, child, right.slice(pos), 1, true, _stack)];
                case 34:
                    endPos = _c.sent();
                    return [4 /*yield*/, saveStack(right.slice(0, pos + endPos), _stack)];
                case 35:
                    _c.sent();
                    return [2 /*return*/, false];
                case 36: throw new Error('invalid node');
            }
        });
    });
}
/**
 * Verifies a proof and return the verified trie.
 * @param rootHash - root hash.
 * @param key - target key.
 * @param proof - proof node list.
 * @throws If proof is found to be invalid.
 * @returns The value from the key, or null if valid proof of non-existence.
 */
function verifyProof(rootHash, key, proof) {
    return __awaiter(this, void 0, void 0, function () {
        var proofTrie, e_1, value, err_1;
        return __generator(this, function (_a) {
            switch (_a.label) {
                case 0:
                    proofTrie = new baseTrie_1.Trie(null, rootHash);
                    _a.label = 1;
                case 1:
                    _a.trys.push([1, 3, , 4]);
                    return [4 /*yield*/, baseTrie_1.Trie.fromProof(proof, proofTrie)];
                case 2:
                    proofTrie = _a.sent();
                    return [3 /*break*/, 4];
                case 3:
                    e_1 = _a.sent();
                    throw new Error('Invalid proof nodes given');
                case 4:
                    _a.trys.push([4, 6, , 7]);
                    return [4 /*yield*/, proofTrie.get(key, true)];
                case 5:
                    value = _a.sent();
                    return [2 /*return*/, {
                            trie: proofTrie,
                            value: value,
                        }];
                case 6:
                    err_1 = _a.sent();
                    if (err_1.message == 'Missing node in DB') {
                        throw new Error('Invalid proof provided');
                    }
                    else {
                        throw err_1;
                    }
                    return [3 /*break*/, 7];
                case 7: return [2 /*return*/];
            }
        });
    });
}
/**
 * hasRightElement returns the indicator whether there exists more elements
 * on the right side of the given path
 * @param trie - trie object.
 * @param key - given path.
 */
function hasRightElement(trie, key) {
    return __awaiter(this, void 0, void 0, function () {
        var pos, node, i, next, _a;
        return __generator(this, function (_b) {
            switch (_b.label) {
                case 0:
                    pos = 0;
                    return [4 /*yield*/, trie.lookupNode(trie.root)];
                case 1:
                    node = _b.sent();
                    _b.label = 2;
                case 2:
                    if (!(node !== null)) return [3 /*break*/, 9];
                    if (!(node instanceof trieNode_1.BranchNode)) return [3 /*break*/, 5];
                    for (i = key[pos] + 1; i < 16; i++) {
                        if (node.getBranch(i) !== null) {
                            return [2 /*return*/, true];
                        }
                    }
                    next = node.getBranch(key[pos]);
                    _a = next;
                    if (!_a) return [3 /*break*/, 4];
                    return [4 /*yield*/, trie.lookupNode(next)];
                case 3:
                    _a = (_b.sent());
                    _b.label = 4;
                case 4:
                    node = _a;
                    pos += 1;
                    return [3 /*break*/, 8];
                case 5:
                    if (!(node instanceof trieNode_1.ExtensionNode)) return [3 /*break*/, 7];
                    if (key.length - pos < node.keyLength ||
                        (0, nibbles_1.nibblesCompare)(node._nibbles, key.slice(pos, pos + node.keyLength)) !== 0) {
                        return [2 /*return*/, (0, nibbles_1.nibblesCompare)(node._nibbles, key.slice(pos)) > 0];
                    }
                    pos += node.keyLength;
                    return [4 /*yield*/, trie.lookupNode(node._value)];
                case 6:
                    node = _b.sent();
                    return [3 /*break*/, 8];
                case 7:
                    if (node instanceof trieNode_1.LeafNode) {
                        return [2 /*return*/, false];
                    }
                    else {
                        throw new Error('invalid node');
                    }
                    _b.label = 8;
                case 8: return [3 /*break*/, 2];
                case 9: return [2 /*return*/, false];
            }
        });
    });
}
/**
 * verifyRangeProof checks whether the given leaf nodes and edge proof
 * can prove the given trie leaves range is matched with the specific root.
 *
 * There are four situations:
 *
 * - All elements proof. In this case the proof can be null, but the range should
 *   be all the leaves in the trie.
 *
 * - One element proof. In this case no matter the edge proof is a non-existent
 *   proof or not, we can always verify the correctness of the proof.
 *
 * - Zero element proof. In this case a single non-existent proof is enough to prove.
 *   Besides, if there are still some other leaves available on the right side, then
 *   an error will be returned.
 *
 * - Two edge elements proof. In this case two existent or non-existent proof(fisrt and last) should be provided.
 *
 * NOTE: Currently only supports verification when the length of firstKey and lastKey are the same.
 *
 * @param rootHash - root hash.
 * @param firstKey - first key.
 * @param lastKey - last key.
 * @param keys - key list.
 * @param values - value list, one-to-one correspondence with keys.
 * @param proof - proof node list, if proof is null, both `firstKey` and `lastKey` must be null
 * @returns a flag to indicate whether there exists more trie node in the trie
 */
function verifyRangeProof(rootHash, firstKey, lastKey, keys, values, proof) {
    return __awaiter(this, void 0, void 0, function () {
        var i, values_1, values_1_1, value, trie_1, i, _a, trie_2, value, _b, _c, trie_3, value, trie, empty, i;
        var e_2, _d;
        return __generator(this, function (_e) {
            switch (_e.label) {
                case 0:
                    if (keys.length !== values.length) {
                        throw new Error('invalid keys length or values length');
                    }
                    // Make sure the keys are in order
                    for (i = 0; i < keys.length - 1; i++) {
                        if ((0, nibbles_1.nibblesCompare)(keys[i], keys[i + 1]) >= 0) {
                            throw new Error('invalid keys order');
                        }
                    }
                    try {
                        // Make sure all values are present
                        for (values_1 = __values(values), values_1_1 = values_1.next(); !values_1_1.done; values_1_1 = values_1.next()) {
                            value = values_1_1.value;
                            if (value.length === 0) {
                                throw new Error('invalid values');
                            }
                        }
                    }
                    catch (e_2_1) { e_2 = { error: e_2_1 }; }
                    finally {
                        try {
                            if (values_1_1 && !values_1_1.done && (_d = values_1.return)) _d.call(values_1);
                        }
                        finally { if (e_2) throw e_2.error; }
                    }
                    if (!(proof === null && firstKey === null && lastKey === null)) return [3 /*break*/, 5];
                    trie_1 = new baseTrie_1.Trie();
                    i = 0;
                    _e.label = 1;
                case 1:
                    if (!(i < keys.length)) return [3 /*break*/, 4];
                    return [4 /*yield*/, trie_1.put((0, nibbles_1.nibblesToBuffer)(keys[i]), values[i])];
                case 2:
                    _e.sent();
                    _e.label = 3;
                case 3:
                    i++;
                    return [3 /*break*/, 1];
                case 4:
                    if (rootHash.compare(trie_1.root) !== 0) {
                        throw new Error('invalid all elements proof: root mismatch');
                    }
                    return [2 /*return*/, false];
                case 5:
                    if (proof === null || firstKey === null || lastKey === null) {
                        throw new Error('invalid all elements proof: proof, firstKey, lastKey must be null at the same time');
                    }
                    if (!(keys.length === 0)) return [3 /*break*/, 9];
                    return [4 /*yield*/, verifyProof(rootHash, (0, nibbles_1.nibblesToBuffer)(firstKey), proof)];
                case 6:
                    _a = _e.sent(), trie_2 = _a.trie, value = _a.value;
                    _b = value !== null;
                    if (_b) return [3 /*break*/, 8];
                    return [4 /*yield*/, hasRightElement(trie_2, firstKey)];
                case 7:
                    _b = (_e.sent());
                    _e.label = 8;
                case 8:
                    if (_b) {
                        throw new Error('invalid zero element proof: value mismatch');
                    }
                    return [2 /*return*/, false];
                case 9:
                    if (!(keys.length === 1 && (0, nibbles_1.nibblesCompare)(firstKey, lastKey) === 0)) return [3 /*break*/, 11];
                    return [4 /*yield*/, verifyProof(rootHash, (0, nibbles_1.nibblesToBuffer)(firstKey), proof)];
                case 10:
                    _c = _e.sent(), trie_3 = _c.trie, value = _c.value;
                    if ((0, nibbles_1.nibblesCompare)(firstKey, keys[0]) !== 0) {
                        throw new Error('invalid one element proof: firstKey should be equal to keys[0]');
                    }
                    if (value === null || value.compare(values[0]) !== 0) {
                        throw new Error('invalid one element proof: value mismatch');
                    }
                    return [2 /*return*/, hasRightElement(trie_3, firstKey)];
                case 11:
                    // Two edge elements proof
                    if ((0, nibbles_1.nibblesCompare)(firstKey, lastKey) >= 0) {
                        throw new Error('invalid two edge elements proof: firstKey should be less than lastKey');
                    }
                    if (firstKey.length !== lastKey.length) {
                        throw new Error('invalid two edge elements proof: the length of firstKey should be equal to the length of lastKey');
                    }
                    trie = new baseTrie_1.Trie(null, rootHash);
                    return [4 /*yield*/, baseTrie_1.Trie.fromProof(proof, trie)
                        // Remove all nodes between two edge proofs
                    ];
                case 12:
                    trie = _e.sent();
                    return [4 /*yield*/, unsetInternal(trie, firstKey, lastKey)];
                case 13:
                    empty = _e.sent();
                    if (empty) {
                        trie.root = trie.EMPTY_TRIE_ROOT;
                    }
                    i = 0;
                    _e.label = 14;
                case 14:
                    if (!(i < keys.length)) return [3 /*break*/, 17];
                    return [4 /*yield*/, trie.put((0, nibbles_1.nibblesToBuffer)(keys[i]), values[i])];
                case 15:
                    _e.sent();
                    _e.label = 16;
                case 16:
                    i++;
                    return [3 /*break*/, 14];
                case 17:
                    // Compare rootHash
                    if (trie.root.compare(rootHash) !== 0) {
                        throw new Error('invalid two edge elements proof: root mismatch');
                    }
                    return [2 /*return*/, hasRightElement(trie, keys[keys.length - 1])];
            }
        });
    });
}
exports.verifyRangeProof = verifyRangeProof;
//# sourceMappingURL=verifyRangeProof.js.map