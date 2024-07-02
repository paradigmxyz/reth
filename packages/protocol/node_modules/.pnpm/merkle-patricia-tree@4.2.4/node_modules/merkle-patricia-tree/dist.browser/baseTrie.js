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
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.Trie = void 0;
var semaphore_async_await_1 = __importDefault(require("semaphore-async-await"));
var ethereumjs_util_1 = require("ethereumjs-util");
var db_1 = require("./db");
var readStream_1 = require("./readStream");
var nibbles_1 = require("./util/nibbles");
var walkController_1 = require("./util/walkController");
var trieNode_1 = require("./trieNode");
var verifyRangeProof_1 = require("./verifyRangeProof");
var assert = require('assert');
/**
 * The basic trie interface, use with `import { BaseTrie as Trie } from 'merkle-patricia-tree'`.
 * In Ethereum applications stick with the {@link SecureTrie} overlay.
 * The API for the base and the secure interface are about the same.
 */
var Trie = /** @class */ (function () {
    /**
     * test
     * @param db - A [levelup](https://github.com/Level/levelup) instance. By default (if the db is `null` or
     * left undefined) creates an in-memory [memdown](https://github.com/Level/memdown) instance.
     * @param root - A `Buffer` for the root of a previously stored trie
     * @param deleteFromDB - Delete nodes from DB on delete operations (disallows switching to an older state root) (default: `false`)
     */
    function Trie(db, root, deleteFromDB) {
        if (deleteFromDB === void 0) { deleteFromDB = false; }
        this.EMPTY_TRIE_ROOT = ethereumjs_util_1.KECCAK256_RLP;
        this.lock = new semaphore_async_await_1.default(1);
        this.db = db ? new db_1.DB(db) : new db_1.DB();
        this._root = this.EMPTY_TRIE_ROOT;
        this._deleteFromDB = deleteFromDB;
        if (root) {
            this.root = root;
        }
    }
    Object.defineProperty(Trie.prototype, "root", {
        /**
         * Gets the current root of the `trie`
         */
        get: function () {
            return this._root;
        },
        /**
         * Sets the current root of the `trie`
         */
        set: function (value) {
            if (!value) {
                value = this.EMPTY_TRIE_ROOT;
            }
            assert(value.length === 32, 'Invalid root length. Roots are 32 bytes');
            this._root = value;
        },
        enumerable: false,
        configurable: true
    });
    /**
     * This method is deprecated.
     * Please use {@link Trie.root} instead.
     *
     * @param value
     * @deprecated
     */
    Trie.prototype.setRoot = function (value) {
        this.root = value !== null && value !== void 0 ? value : this.EMPTY_TRIE_ROOT;
    };
    /**
     * Checks if a given root exists.
     */
    Trie.prototype.checkRoot = function (root) {
        return __awaiter(this, void 0, void 0, function () {
            var value, error_1;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        _a.trys.push([0, 2, , 3]);
                        return [4 /*yield*/, this._lookupNode(root)];
                    case 1:
                        value = _a.sent();
                        return [2 /*return*/, value !== null];
                    case 2:
                        error_1 = _a.sent();
                        if (error_1.message == 'Missing node in DB') {
                            return [2 /*return*/, false];
                        }
                        else {
                            throw error_1;
                        }
                        return [3 /*break*/, 3];
                    case 3: return [2 /*return*/];
                }
            });
        });
    };
    Object.defineProperty(Trie.prototype, "isCheckpoint", {
        /**
         * BaseTrie has no checkpointing so return false
         */
        get: function () {
            return false;
        },
        enumerable: false,
        configurable: true
    });
    /**
     * Gets a value given a `key`
     * @param key - the key to search for
     * @param throwIfMissing - if true, throws if any nodes are missing. Used for verifying proofs. (default: false)
     * @returns A Promise that resolves to `Buffer` if a value was found or `null` if no value was found.
     */
    Trie.prototype.get = function (key, throwIfMissing) {
        if (throwIfMissing === void 0) { throwIfMissing = false; }
        return __awaiter(this, void 0, void 0, function () {
            var _a, node, remaining, value;
            return __generator(this, function (_b) {
                switch (_b.label) {
                    case 0: return [4 /*yield*/, this.findPath(key, throwIfMissing)];
                    case 1:
                        _a = _b.sent(), node = _a.node, remaining = _a.remaining;
                        value = null;
                        if (node && remaining.length === 0) {
                            value = node.value;
                        }
                        return [2 /*return*/, value];
                }
            });
        });
    };
    /**
     * Stores a given `value` at the given `key` or do a delete if `value` is empty
     * (delete operations are only executed on DB with `deleteFromDB` set to `true`)
     * @param key
     * @param value
     * @returns A Promise that resolves once value is stored.
     */
    Trie.prototype.put = function (key, value) {
        return __awaiter(this, void 0, void 0, function () {
            var _a, remaining, stack;
            return __generator(this, function (_b) {
                switch (_b.label) {
                    case 0:
                        if (!(!value || value.toString() === '')) return [3 /*break*/, 2];
                        return [4 /*yield*/, this.del(key)];
                    case 1: return [2 /*return*/, _b.sent()];
                    case 2: return [4 /*yield*/, this.lock.wait()];
                    case 3:
                        _b.sent();
                        if (!this.root.equals(ethereumjs_util_1.KECCAK256_RLP)) return [3 /*break*/, 5];
                        // If no root, initialize this trie
                        return [4 /*yield*/, this._createInitialNode(key, value)];
                    case 4:
                        // If no root, initialize this trie
                        _b.sent();
                        return [3 /*break*/, 8];
                    case 5: return [4 /*yield*/, this.findPath(key)
                        // then update
                    ];
                    case 6:
                        _a = _b.sent(), remaining = _a.remaining, stack = _a.stack;
                        // then update
                        return [4 /*yield*/, this._updateNode(key, value, remaining, stack)];
                    case 7:
                        // then update
                        _b.sent();
                        _b.label = 8;
                    case 8:
                        this.lock.signal();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Deletes a value given a `key` from the trie
     * (delete operations are only executed on DB with `deleteFromDB` set to `true`)
     * @param key
     * @returns A Promise that resolves once value is deleted.
     */
    Trie.prototype.del = function (key) {
        return __awaiter(this, void 0, void 0, function () {
            var _a, node, stack;
            return __generator(this, function (_b) {
                switch (_b.label) {
                    case 0: return [4 /*yield*/, this.lock.wait()];
                    case 1:
                        _b.sent();
                        return [4 /*yield*/, this.findPath(key)];
                    case 2:
                        _a = _b.sent(), node = _a.node, stack = _a.stack;
                        if (!node) return [3 /*break*/, 4];
                        return [4 /*yield*/, this._deleteNode(key, stack)];
                    case 3:
                        _b.sent();
                        _b.label = 4;
                    case 4:
                        this.lock.signal();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Tries to find a path to the node for the given key.
     * It returns a `stack` of nodes to the closest node.
     * @param key - the search key
     * @param throwIfMissing - if true, throws if any nodes are missing. Used for verifying proofs. (default: false)
     */
    Trie.prototype.findPath = function (key, throwIfMissing) {
        if (throwIfMissing === void 0) { throwIfMissing = false; }
        return __awaiter(this, void 0, void 0, function () {
            var _this = this;
            return __generator(this, function (_a) {
                // eslint-disable-next-line no-async-promise-executor
                return [2 /*return*/, new Promise(function (resolve, reject) { return __awaiter(_this, void 0, void 0, function () {
                        var stack, targetKey, onFound, error_2;
                        var _this = this;
                        return __generator(this, function (_a) {
                            switch (_a.label) {
                                case 0:
                                    stack = [];
                                    targetKey = (0, nibbles_1.bufferToNibbles)(key);
                                    onFound = function (nodeRef, node, keyProgress, walkController) { return __awaiter(_this, void 0, void 0, function () {
                                        var keyRemainder, branchIndex, branchNode, matchingLen;
                                        return __generator(this, function (_a) {
                                            if (node === null) {
                                                return [2 /*return*/, reject(new Error('Path not found'))];
                                            }
                                            keyRemainder = targetKey.slice((0, nibbles_1.matchingNibbleLength)(keyProgress, targetKey));
                                            stack.push(node);
                                            if (node instanceof trieNode_1.BranchNode) {
                                                if (keyRemainder.length === 0) {
                                                    // we exhausted the key without finding a node
                                                    resolve({ node: node, remaining: [], stack: stack });
                                                }
                                                else {
                                                    branchIndex = keyRemainder[0];
                                                    branchNode = node.getBranch(branchIndex);
                                                    if (!branchNode) {
                                                        // there are no more nodes to find and we didn't find the key
                                                        resolve({ node: null, remaining: keyRemainder, stack: stack });
                                                    }
                                                    else {
                                                        // node found, continuing search
                                                        // this can be optimized as this calls getBranch again.
                                                        walkController.onlyBranchIndex(node, keyProgress, branchIndex);
                                                    }
                                                }
                                            }
                                            else if (node instanceof trieNode_1.LeafNode) {
                                                if ((0, nibbles_1.doKeysMatch)(keyRemainder, node.key)) {
                                                    // keys match, return node with empty key
                                                    resolve({ node: node, remaining: [], stack: stack });
                                                }
                                                else {
                                                    // reached leaf but keys dont match
                                                    resolve({ node: null, remaining: keyRemainder, stack: stack });
                                                }
                                            }
                                            else if (node instanceof trieNode_1.ExtensionNode) {
                                                matchingLen = (0, nibbles_1.matchingNibbleLength)(keyRemainder, node.key);
                                                if (matchingLen !== node.key.length) {
                                                    // keys don't match, fail
                                                    resolve({ node: null, remaining: keyRemainder, stack: stack });
                                                }
                                                else {
                                                    // keys match, continue search
                                                    walkController.allChildren(node, keyProgress);
                                                }
                                            }
                                            return [2 /*return*/];
                                        });
                                    }); };
                                    _a.label = 1;
                                case 1:
                                    _a.trys.push([1, 3, , 4]);
                                    return [4 /*yield*/, this.walkTrie(this.root, onFound)];
                                case 2:
                                    _a.sent();
                                    return [3 /*break*/, 4];
                                case 3:
                                    error_2 = _a.sent();
                                    if (error_2.message == 'Missing node in DB' && !throwIfMissing) {
                                        // pass
                                    }
                                    else {
                                        reject(error_2);
                                    }
                                    return [3 /*break*/, 4];
                                case 4:
                                    // Resolve if _walkTrie finishes without finding any nodes
                                    resolve({ node: null, remaining: [], stack: stack });
                                    return [2 /*return*/];
                            }
                        });
                    }); })];
            });
        });
    };
    /**
     * Walks a trie until finished.
     * @param root
     * @param onFound - callback to call when a node is found. This schedules new tasks. If no tasks are available, the Promise resolves.
     * @returns Resolves when finished walking trie.
     */
    Trie.prototype.walkTrie = function (root, onFound) {
        return __awaiter(this, void 0, void 0, function () {
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0: return [4 /*yield*/, walkController_1.WalkController.newWalk(onFound, this, root)];
                    case 1:
                        _a.sent();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * @hidden
     * Backwards compatibility
     * @param root -
     * @param onFound -
     */
    Trie.prototype._walkTrie = function (root, onFound) {
        return __awaiter(this, void 0, void 0, function () {
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0: return [4 /*yield*/, this.walkTrie(root, onFound)];
                    case 1:
                        _a.sent();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Creates the initial node from an empty tree.
     * @private
     */
    Trie.prototype._createInitialNode = function (key, value) {
        return __awaiter(this, void 0, void 0, function () {
            var newNode;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        newNode = new trieNode_1.LeafNode((0, nibbles_1.bufferToNibbles)(key), value);
                        this.root = newNode.hash();
                        return [4 /*yield*/, this.db.put(this.root, newNode.serialize())];
                    case 1:
                        _a.sent();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Retrieves a node from db by hash.
     */
    Trie.prototype.lookupNode = function (node) {
        return __awaiter(this, void 0, void 0, function () {
            var value, foundNode;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        if ((0, trieNode_1.isRawNode)(node)) {
                            return [2 /*return*/, (0, trieNode_1.decodeRawNode)(node)];
                        }
                        value = null;
                        foundNode = null;
                        return [4 /*yield*/, this.db.get(node)];
                    case 1:
                        value = _a.sent();
                        if (value) {
                            foundNode = (0, trieNode_1.decodeNode)(value);
                        }
                        else {
                            // Dev note: this error message text is used for error checking in `checkRoot`, `verifyProof`, and `findPath`
                            throw new Error('Missing node in DB');
                        }
                        return [2 /*return*/, foundNode];
                }
            });
        });
    };
    /**
     * @hidden
     * Backwards compatibility
     * @param node The node hash to lookup from the DB
     */
    Trie.prototype._lookupNode = function (node) {
        return __awaiter(this, void 0, void 0, function () {
            return __generator(this, function (_a) {
                return [2 /*return*/, this.lookupNode(node)];
            });
        });
    };
    /**
     * Updates a node.
     * @private
     * @param key
     * @param value
     * @param keyRemainder
     * @param stack
     */
    Trie.prototype._updateNode = function (k, value, keyRemainder, stack) {
        return __awaiter(this, void 0, void 0, function () {
            var toSave, lastNode, key, matchLeaf, l, i, n, newLeaf, lastKey, matchingLength, newBranchNode, newKey, newExtNode, branchKey, formattedNode, newLeafNode;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        toSave = [];
                        lastNode = stack.pop();
                        if (!lastNode) {
                            throw new Error('Stack underflow');
                        }
                        key = (0, nibbles_1.bufferToNibbles)(k);
                        matchLeaf = false;
                        if (lastNode instanceof trieNode_1.LeafNode) {
                            l = 0;
                            for (i = 0; i < stack.length; i++) {
                                n = stack[i];
                                if (n instanceof trieNode_1.BranchNode) {
                                    l++;
                                }
                                else {
                                    l += n.key.length;
                                }
                            }
                            if ((0, nibbles_1.matchingNibbleLength)(lastNode.key, key.slice(l)) === lastNode.key.length &&
                                keyRemainder.length === 0) {
                                matchLeaf = true;
                            }
                        }
                        if (matchLeaf) {
                            // just updating a found value
                            lastNode.value = value;
                            stack.push(lastNode);
                        }
                        else if (lastNode instanceof trieNode_1.BranchNode) {
                            stack.push(lastNode);
                            if (keyRemainder.length !== 0) {
                                // add an extension to a branch node
                                keyRemainder.shift();
                                newLeaf = new trieNode_1.LeafNode(keyRemainder, value);
                                stack.push(newLeaf);
                            }
                            else {
                                lastNode.value = value;
                            }
                        }
                        else {
                            lastKey = lastNode.key;
                            matchingLength = (0, nibbles_1.matchingNibbleLength)(lastKey, keyRemainder);
                            newBranchNode = new trieNode_1.BranchNode();
                            // create a new extension node
                            if (matchingLength !== 0) {
                                newKey = lastNode.key.slice(0, matchingLength);
                                newExtNode = new trieNode_1.ExtensionNode(newKey, value);
                                stack.push(newExtNode);
                                lastKey.splice(0, matchingLength);
                                keyRemainder.splice(0, matchingLength);
                            }
                            stack.push(newBranchNode);
                            if (lastKey.length !== 0) {
                                branchKey = lastKey.shift();
                                if (lastKey.length !== 0 || lastNode instanceof trieNode_1.LeafNode) {
                                    // shrinking extension or leaf
                                    lastNode.key = lastKey;
                                    formattedNode = this._formatNode(lastNode, false, toSave);
                                    newBranchNode.setBranch(branchKey, formattedNode);
                                }
                                else {
                                    // remove extension or attaching
                                    this._formatNode(lastNode, false, toSave, true);
                                    newBranchNode.setBranch(branchKey, lastNode.value);
                                }
                            }
                            else {
                                newBranchNode.value = lastNode.value;
                            }
                            if (keyRemainder.length !== 0) {
                                keyRemainder.shift();
                                newLeafNode = new trieNode_1.LeafNode(keyRemainder, value);
                                stack.push(newLeafNode);
                            }
                            else {
                                newBranchNode.value = value;
                            }
                        }
                        return [4 /*yield*/, this._saveStack(key, stack, toSave)];
                    case 1:
                        _a.sent();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Deletes a node from the trie.
     * @private
     */
    Trie.prototype._deleteNode = function (k, stack) {
        return __awaiter(this, void 0, void 0, function () {
            var processBranchNode, lastNode, parentNode, opStack, key, lastNodeKey, branchNodes, branchNode, branchNodeKey, foundNode;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        processBranchNode = function (key, branchKey, branchNode, parentNode, stack) {
                            // branchNode is the node ON the branch node not THE branch node
                            if (!parentNode || parentNode instanceof trieNode_1.BranchNode) {
                                // branch->?
                                if (parentNode) {
                                    stack.push(parentNode);
                                }
                                if (branchNode instanceof trieNode_1.BranchNode) {
                                    // create an extension node
                                    // branch->extension->branch
                                    // @ts-ignore
                                    var extensionNode = new trieNode_1.ExtensionNode([branchKey], null);
                                    stack.push(extensionNode);
                                    key.push(branchKey);
                                }
                                else {
                                    var branchNodeKey = branchNode.key;
                                    // branch key is an extension or a leaf
                                    // branch->(leaf or extension)
                                    branchNodeKey.unshift(branchKey);
                                    branchNode.key = branchNodeKey.slice(0);
                                    key = key.concat(branchNodeKey);
                                }
                                stack.push(branchNode);
                            }
                            else {
                                // parent is an extension
                                var parentKey = parentNode.key;
                                if (branchNode instanceof trieNode_1.BranchNode) {
                                    // ext->branch
                                    parentKey.push(branchKey);
                                    key.push(branchKey);
                                    parentNode.key = parentKey;
                                    stack.push(parentNode);
                                }
                                else {
                                    var branchNodeKey = branchNode.key;
                                    // branch node is an leaf or extension and parent node is an exstention
                                    // add two keys together
                                    // dont push the parent node
                                    branchNodeKey.unshift(branchKey);
                                    key = key.concat(branchNodeKey);
                                    parentKey = parentKey.concat(branchNodeKey);
                                    branchNode.key = parentKey;
                                }
                                stack.push(branchNode);
                            }
                            return key;
                        };
                        lastNode = stack.pop();
                        assert(lastNode);
                        parentNode = stack.pop();
                        opStack = [];
                        key = (0, nibbles_1.bufferToNibbles)(k);
                        if (!parentNode) {
                            // the root here has to be a leaf.
                            this.root = this.EMPTY_TRIE_ROOT;
                            return [2 /*return*/];
                        }
                        if (lastNode instanceof trieNode_1.BranchNode) {
                            lastNode.value = null;
                        }
                        else {
                            // the lastNode has to be a leaf if it's not a branch.
                            // And a leaf's parent, if it has one, must be a branch.
                            if (!(parentNode instanceof trieNode_1.BranchNode)) {
                                throw new Error('Expected branch node');
                            }
                            lastNodeKey = lastNode.key;
                            key.splice(key.length - lastNodeKey.length);
                            // delete the value
                            this._formatNode(lastNode, false, opStack, true);
                            parentNode.setBranch(key.pop(), null);
                            lastNode = parentNode;
                            parentNode = stack.pop();
                        }
                        branchNodes = lastNode.getChildren();
                        if (!(branchNodes.length === 1)) return [3 /*break*/, 4];
                        branchNode = branchNodes[0][1];
                        branchNodeKey = branchNodes[0][0];
                        return [4 /*yield*/, this._lookupNode(branchNode)];
                    case 1:
                        foundNode = _a.sent();
                        if (!foundNode) return [3 /*break*/, 3];
                        key = processBranchNode(key, branchNodeKey, foundNode, parentNode, stack);
                        return [4 /*yield*/, this._saveStack(key, stack, opStack)];
                    case 2:
                        _a.sent();
                        _a.label = 3;
                    case 3: return [3 /*break*/, 6];
                    case 4:
                        // simple removing a leaf and recaluclation the stack
                        if (parentNode) {
                            stack.push(parentNode);
                        }
                        stack.push(lastNode);
                        return [4 /*yield*/, this._saveStack(key, stack, opStack)];
                    case 5:
                        _a.sent();
                        _a.label = 6;
                    case 6: return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Saves a stack of nodes to the database.
     * @private
     * @param key - the key. Should follow the stack
     * @param stack - a stack of nodes to the value given by the key
     * @param opStack - a stack of levelup operations to commit at the end of this funciton
     */
    Trie.prototype._saveStack = function (key, stack, opStack) {
        return __awaiter(this, void 0, void 0, function () {
            var lastRoot, node, branchKey;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        // update nodes
                        while (stack.length) {
                            node = stack.pop();
                            if (node instanceof trieNode_1.LeafNode) {
                                key.splice(key.length - node.key.length);
                            }
                            else if (node instanceof trieNode_1.ExtensionNode) {
                                key.splice(key.length - node.key.length);
                                if (lastRoot) {
                                    node.value = lastRoot;
                                }
                            }
                            else if (node instanceof trieNode_1.BranchNode) {
                                if (lastRoot) {
                                    branchKey = key.pop();
                                    node.setBranch(branchKey, lastRoot);
                                }
                            }
                            lastRoot = this._formatNode(node, stack.length === 0, opStack);
                        }
                        if (lastRoot) {
                            this.root = lastRoot;
                        }
                        return [4 /*yield*/, this.db.batch(opStack)];
                    case 1:
                        _a.sent();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Formats node to be saved by `levelup.batch`.
     * @private
     * @param node - the node to format.
     * @param topLevel - if the node is at the top level.
     * @param opStack - the opStack to push the node's data.
     * @param remove - whether to remove the node (only used for CheckpointTrie).
     * @returns The node's hash used as the key or the rawNode.
     */
    Trie.prototype._formatNode = function (node, topLevel, opStack, remove) {
        if (remove === void 0) { remove = false; }
        var rlpNode = node.serialize();
        if (rlpNode.length >= 32 || topLevel) {
            // Do not use TrieNode.hash() here otherwise serialize()
            // is applied twice (performance)
            var hashRoot = (0, ethereumjs_util_1.keccak)(rlpNode);
            if (remove) {
                if (this._deleteFromDB) {
                    opStack.push({
                        type: 'del',
                        key: hashRoot,
                    });
                }
            }
            else {
                opStack.push({
                    type: 'put',
                    key: hashRoot,
                    value: rlpNode,
                });
            }
            return hashRoot;
        }
        return node.raw();
    };
    /**
     * The given hash of operations (key additions or deletions) are executed on the trie
     * (delete operations are only executed on DB with `deleteFromDB` set to `true`)
     * @example
     * const ops = [
     *    { type: 'del', key: Buffer.from('father') }
     *  , { type: 'put', key: Buffer.from('name'), value: Buffer.from('Yuri Irsenovich Kim') }
     *  , { type: 'put', key: Buffer.from('dob'), value: Buffer.from('16 February 1941') }
     *  , { type: 'put', key: Buffer.from('spouse'), value: Buffer.from('Kim Young-sook') }
     *  , { type: 'put', key: Buffer.from('occupation'), value: Buffer.from('Clown') }
     * ]
     * await trie.batch(ops)
     * @param ops
     */
    Trie.prototype.batch = function (ops) {
        return __awaiter(this, void 0, void 0, function () {
            var ops_1, ops_1_1, op, e_1_1;
            var e_1, _a;
            return __generator(this, function (_b) {
                switch (_b.label) {
                    case 0:
                        _b.trys.push([0, 7, 8, 9]);
                        ops_1 = __values(ops), ops_1_1 = ops_1.next();
                        _b.label = 1;
                    case 1:
                        if (!!ops_1_1.done) return [3 /*break*/, 6];
                        op = ops_1_1.value;
                        if (!(op.type === 'put')) return [3 /*break*/, 3];
                        if (!op.value) {
                            throw new Error('Invalid batch db operation');
                        }
                        return [4 /*yield*/, this.put(op.key, op.value)];
                    case 2:
                        _b.sent();
                        return [3 /*break*/, 5];
                    case 3:
                        if (!(op.type === 'del')) return [3 /*break*/, 5];
                        return [4 /*yield*/, this.del(op.key)];
                    case 4:
                        _b.sent();
                        _b.label = 5;
                    case 5:
                        ops_1_1 = ops_1.next();
                        return [3 /*break*/, 1];
                    case 6: return [3 /*break*/, 9];
                    case 7:
                        e_1_1 = _b.sent();
                        e_1 = { error: e_1_1 };
                        return [3 /*break*/, 9];
                    case 8:
                        try {
                            if (ops_1_1 && !ops_1_1.done && (_a = ops_1.return)) _a.call(ops_1);
                        }
                        finally { if (e_1) throw e_1.error; }
                        return [7 /*endfinally*/];
                    case 9: return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Saves the nodes from a proof into the trie. If no trie is provided a new one wil be instantiated.
     * @param proof
     * @param trie
     */
    Trie.fromProof = function (proof, trie) {
        return __awaiter(this, void 0, void 0, function () {
            var opStack;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        opStack = proof.map(function (nodeValue) {
                            return {
                                type: 'put',
                                key: (0, ethereumjs_util_1.keccak)(nodeValue),
                                value: nodeValue,
                            };
                        });
                        if (!trie) {
                            trie = new Trie();
                            if (opStack[0]) {
                                trie.root = opStack[0].key;
                            }
                        }
                        return [4 /*yield*/, trie.db.batch(opStack)];
                    case 1:
                        _a.sent();
                        return [2 /*return*/, trie];
                }
            });
        });
    };
    /**
     * prove has been renamed to {@link Trie.createProof}.
     * @deprecated
     * @param trie
     * @param key
     */
    Trie.prove = function (trie, key) {
        return __awaiter(this, void 0, void 0, function () {
            return __generator(this, function (_a) {
                return [2 /*return*/, this.createProof(trie, key)];
            });
        });
    };
    /**
     * Creates a proof from a trie and key that can be verified using {@link Trie.verifyProof}.
     * @param trie
     * @param key
     */
    Trie.createProof = function (trie, key) {
        return __awaiter(this, void 0, void 0, function () {
            var stack, p;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0: return [4 /*yield*/, trie.findPath(key)];
                    case 1:
                        stack = (_a.sent()).stack;
                        p = stack.map(function (stackElem) {
                            return stackElem.serialize();
                        });
                        return [2 /*return*/, p];
                }
            });
        });
    };
    /**
     * Verifies a proof.
     * @param rootHash
     * @param key
     * @param proof
     * @throws If proof is found to be invalid.
     * @returns The value from the key, or null if valid proof of non-existence.
     */
    Trie.verifyProof = function (rootHash, key, proof) {
        return __awaiter(this, void 0, void 0, function () {
            var proofTrie, e_2, value, err_1;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        proofTrie = new Trie(null, rootHash);
                        _a.label = 1;
                    case 1:
                        _a.trys.push([1, 3, , 4]);
                        return [4 /*yield*/, Trie.fromProof(proof, proofTrie)];
                    case 2:
                        proofTrie = _a.sent();
                        return [3 /*break*/, 4];
                    case 3:
                        e_2 = _a.sent();
                        throw new Error('Invalid proof nodes given');
                    case 4:
                        _a.trys.push([4, 6, , 7]);
                        return [4 /*yield*/, proofTrie.get(key, true)];
                    case 5:
                        value = _a.sent();
                        return [2 /*return*/, value];
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
    };
    /**
     * {@link verifyRangeProof}
     */
    Trie.verifyRangeProof = function (rootHash, firstKey, lastKey, keys, values, proof) {
        return (0, verifyRangeProof_1.verifyRangeProof)(rootHash, firstKey && (0, nibbles_1.bufferToNibbles)(firstKey), lastKey && (0, nibbles_1.bufferToNibbles)(lastKey), keys.map(nibbles_1.bufferToNibbles), values, proof);
    };
    /**
     * The `data` event is given an `Object` that has two properties; the `key` and the `value`. Both should be Buffers.
     * @return Returns a [stream](https://nodejs.org/dist/latest-v12.x/docs/api/stream.html#stream_class_stream_readable) of the contents of the `trie`
     */
    Trie.prototype.createReadStream = function () {
        return new readStream_1.TrieReadStream(this);
    };
    /**
     * Creates a new trie backed by the same db.
     */
    Trie.prototype.copy = function () {
        var db = this.db.copy();
        return new Trie(db._leveldb, this.root);
    };
    /**
     * Finds all nodes that are stored directly in the db
     * (some nodes are stored raw inside other nodes)
     * called by {@link ScratchReadStream}
     * @private
     */
    Trie.prototype._findDbNodes = function (onFound) {
        return __awaiter(this, void 0, void 0, function () {
            var outerOnFound;
            var _this = this;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        outerOnFound = function (nodeRef, node, key, walkController) { return __awaiter(_this, void 0, void 0, function () {
                            return __generator(this, function (_a) {
                                if ((0, trieNode_1.isRawNode)(nodeRef)) {
                                    if (node !== null) {
                                        walkController.allChildren(node, key);
                                    }
                                }
                                else {
                                    onFound(nodeRef, node, key, walkController);
                                }
                                return [2 /*return*/];
                            });
                        }); };
                        return [4 /*yield*/, this.walkTrie(this.root, outerOnFound)];
                    case 1:
                        _a.sent();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Finds all nodes that store k,v values
     * called by {@link TrieReadStream}
     * @private
     */
    Trie.prototype._findValueNodes = function (onFound) {
        return __awaiter(this, void 0, void 0, function () {
            var outerOnFound;
            var _this = this;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        outerOnFound = function (nodeRef, node, key, walkController) { return __awaiter(_this, void 0, void 0, function () {
                            var fullKey;
                            return __generator(this, function (_a) {
                                fullKey = key;
                                if (node instanceof trieNode_1.LeafNode) {
                                    fullKey = key.concat(node.key);
                                    // found leaf node!
                                    onFound(nodeRef, node, fullKey, walkController);
                                }
                                else if (node instanceof trieNode_1.BranchNode && node.value) {
                                    // found branch with value
                                    onFound(nodeRef, node, fullKey, walkController);
                                }
                                else {
                                    // keep looking for value nodes
                                    if (node !== null) {
                                        walkController.allChildren(node, key);
                                    }
                                }
                                return [2 /*return*/];
                            });
                        }); };
                        return [4 /*yield*/, this.walkTrie(this.root, outerOnFound)];
                    case 1:
                        _a.sent();
                        return [2 /*return*/];
                }
            });
        });
    };
    return Trie;
}());
exports.Trie = Trie;
//# sourceMappingURL=baseTrie.js.map