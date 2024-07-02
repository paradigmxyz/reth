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
Object.defineProperty(exports, "__esModule", { value: true });
exports.WalkController = void 0;
var prioritizedTaskExecutor_1 = require("../prioritizedTaskExecutor");
var trieNode_1 = require("../trieNode");
/**
 * WalkController is an interface to control how the trie is being traversed.
 */
var WalkController = /** @class */ (function () {
    /**
     * Creates a new WalkController
     * @param onNode - The `FoundNodeFunction` to call if a node is found.
     * @param trie - The `Trie` to walk on.
     * @param poolSize - The size of the task queue.
     */
    function WalkController(onNode, trie, poolSize) {
        this.onNode = onNode;
        this.taskExecutor = new prioritizedTaskExecutor_1.PrioritizedTaskExecutor(poolSize);
        this.trie = trie;
        this.resolve = function () { };
        this.reject = function () { };
    }
    /**
     * Async function to create and start a new walk over a trie.
     * @param onNode - The `FoundNodeFunction to call if a node is found.
     * @param trie - The trie to walk on.
     * @param root - The root key to walk on.
     * @param poolSize - Task execution pool size to prevent OOM errors. Defaults to 500.
     */
    WalkController.newWalk = function (onNode, trie, root, poolSize) {
        return __awaiter(this, void 0, void 0, function () {
            var strategy;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        strategy = new WalkController(onNode, trie, poolSize !== null && poolSize !== void 0 ? poolSize : 500);
                        return [4 /*yield*/, strategy.startWalk(root)];
                    case 1:
                        _a.sent();
                        return [2 /*return*/];
                }
            });
        });
    };
    WalkController.prototype.startWalk = function (root) {
        return __awaiter(this, void 0, void 0, function () {
            var _this = this;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0: return [4 /*yield*/, new Promise(function (resolve, reject) { return __awaiter(_this, void 0, void 0, function () {
                            var node, error_1;
                            return __generator(this, function (_a) {
                                switch (_a.label) {
                                    case 0:
                                        this.resolve = resolve;
                                        this.reject = reject;
                                        _a.label = 1;
                                    case 1:
                                        _a.trys.push([1, 3, , 4]);
                                        return [4 /*yield*/, this.trie._lookupNode(root)];
                                    case 2:
                                        node = _a.sent();
                                        return [3 /*break*/, 4];
                                    case 3:
                                        error_1 = _a.sent();
                                        return [2 /*return*/, this.reject(error_1)];
                                    case 4:
                                        this.processNode(root, node, []);
                                        return [2 /*return*/];
                                }
                            });
                        }); })];
                    case 1: 
                    // eslint-disable-next-line no-async-promise-executor
                    return [2 /*return*/, _a.sent()];
                }
            });
        });
    };
    /**
     * Run all children of a node. Priority of these nodes are the key length of the children.
     * @param node - Node to get all children of and call onNode on.
     * @param key - The current `key` which would yield the `node` when trying to get this node with a `get` operation.
     */
    WalkController.prototype.allChildren = function (node, key) {
        var e_1, _a;
        if (key === void 0) { key = []; }
        if (node instanceof trieNode_1.LeafNode) {
            return;
        }
        var children;
        if (node instanceof trieNode_1.ExtensionNode) {
            children = [[node.key, node.value]];
        }
        else if (node instanceof trieNode_1.BranchNode) {
            children = node.getChildren().map(function (b) { return [[b[0]], b[1]]; });
        }
        if (!children) {
            return;
        }
        try {
            for (var children_1 = __values(children), children_1_1 = children_1.next(); !children_1_1.done; children_1_1 = children_1.next()) {
                var child = children_1_1.value;
                var keyExtension = child[0];
                var childRef = child[1];
                var childKey = key.concat(keyExtension);
                var priority = childKey.length;
                this.pushNodeToQueue(childRef, childKey, priority);
            }
        }
        catch (e_1_1) { e_1 = { error: e_1_1 }; }
        finally {
            try {
                if (children_1_1 && !children_1_1.done && (_a = children_1.return)) _a.call(children_1);
            }
            finally { if (e_1) throw e_1.error; }
        }
    };
    /**
     * Push a node to the queue. If the queue has places left for tasks, the node is executed immediately, otherwise it is queued.
     * @param nodeRef - Push a node reference to the event queue. This reference is a 32-byte keccak hash of the value corresponding to the `key`.
     * @param key - The current key.
     * @param priority - Optional priority, defaults to key length
     */
    WalkController.prototype.pushNodeToQueue = function (nodeRef, key, priority) {
        var _this = this;
        if (key === void 0) { key = []; }
        this.taskExecutor.executeOrQueue(priority !== null && priority !== void 0 ? priority : key.length, function (taskFinishedCallback) { return __awaiter(_this, void 0, void 0, function () {
            var childNode, error_2;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        _a.trys.push([0, 2, , 3]);
                        return [4 /*yield*/, this.trie._lookupNode(nodeRef)];
                    case 1:
                        childNode = _a.sent();
                        return [3 /*break*/, 3];
                    case 2:
                        error_2 = _a.sent();
                        return [2 /*return*/, this.reject(error_2)];
                    case 3:
                        taskFinishedCallback(); // this marks the current task as finished. If there are any tasks left in the queue, this will immediately execute the first task.
                        this.processNode(nodeRef, childNode, key);
                        return [2 /*return*/];
                }
            });
        }); });
    };
    /**
     * Push a branch of a certain BranchNode to the event queue.
     * @param node - The node to select a branch on. Should be a BranchNode.
     * @param key - The current key which leads to the corresponding node.
     * @param childIndex - The child index to add to the event queue.
     * @param priority - Optional priority of the event, defaults to the total key length.
     */
    WalkController.prototype.onlyBranchIndex = function (node, key, childIndex, priority) {
        if (key === void 0) { key = []; }
        if (!(node instanceof trieNode_1.BranchNode)) {
            throw new Error('Expected branch node');
        }
        var childRef = node.getBranch(childIndex);
        if (!childRef) {
            throw new Error('Could not get branch of childIndex');
        }
        var childKey = key.slice(); // This copies the key to a new array.
        childKey.push(childIndex);
        var prio = priority !== null && priority !== void 0 ? priority : childKey.length;
        this.pushNodeToQueue(childRef, childKey, prio);
    };
    WalkController.prototype.processNode = function (nodeRef, node, key) {
        if (key === void 0) { key = []; }
        this.onNode(nodeRef, node, key, this);
        if (this.taskExecutor.finished()) {
            // onNode should schedule new tasks. If no tasks was added and the queue is empty, then we have finished our walk.
            this.resolve();
        }
    };
    return WalkController;
}());
exports.WalkController = WalkController;
//# sourceMappingURL=walkController.js.map