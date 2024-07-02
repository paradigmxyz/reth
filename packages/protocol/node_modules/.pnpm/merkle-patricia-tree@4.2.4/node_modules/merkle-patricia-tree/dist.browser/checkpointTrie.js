"use strict";
var __extends = (this && this.__extends) || (function () {
    var extendStatics = function (d, b) {
        extendStatics = Object.setPrototypeOf ||
            ({ __proto__: [] } instanceof Array && function (d, b) { d.__proto__ = b; }) ||
            function (d, b) { for (var p in b) if (Object.prototype.hasOwnProperty.call(b, p)) d[p] = b[p]; };
        return extendStatics(d, b);
    };
    return function (d, b) {
        if (typeof b !== "function" && b !== null)
            throw new TypeError("Class extends value " + String(b) + " is not a constructor or null");
        extendStatics(d, b);
        function __() { this.constructor = d; }
        d.prototype = b === null ? Object.create(b) : (__.prototype = b.prototype, new __());
    };
})();
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
Object.defineProperty(exports, "__esModule", { value: true });
exports.CheckpointTrie = void 0;
var baseTrie_1 = require("./baseTrie");
var checkpointDb_1 = require("./checkpointDb");
/**
 * Adds checkpointing to the {@link BaseTrie}
 */
var CheckpointTrie = /** @class */ (function (_super) {
    __extends(CheckpointTrie, _super);
    function CheckpointTrie() {
        var args = [];
        for (var _i = 0; _i < arguments.length; _i++) {
            args[_i] = arguments[_i];
        }
        var _this = _super.apply(this, __spreadArray([], __read(args), false)) || this;
        _this.db = new (checkpointDb_1.CheckpointDB.bind.apply(checkpointDb_1.CheckpointDB, __spreadArray([void 0], __read(args), false)))();
        return _this;
    }
    Object.defineProperty(CheckpointTrie.prototype, "isCheckpoint", {
        /**
         * Is the trie during a checkpoint phase?
         */
        get: function () {
            return this.db.isCheckpoint;
        },
        enumerable: false,
        configurable: true
    });
    /**
     * Creates a checkpoint that can later be reverted to or committed.
     * After this is called, all changes can be reverted until `commit` is called.
     */
    CheckpointTrie.prototype.checkpoint = function () {
        this.db.checkpoint(this.root);
    };
    /**
     * Commits a checkpoint to disk, if current checkpoint is not nested.
     * If nested, only sets the parent checkpoint as current checkpoint.
     * @throws If not during a checkpoint phase
     */
    CheckpointTrie.prototype.commit = function () {
        return __awaiter(this, void 0, void 0, function () {
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        if (!this.isCheckpoint) {
                            throw new Error('trying to commit when not checkpointed');
                        }
                        return [4 /*yield*/, this.lock.wait()];
                    case 1:
                        _a.sent();
                        return [4 /*yield*/, this.db.commit()];
                    case 2:
                        _a.sent();
                        this.lock.signal();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Reverts the trie to the state it was at when `checkpoint` was first called.
     * If during a nested checkpoint, sets root to most recent checkpoint, and sets
     * parent checkpoint as current.
     */
    CheckpointTrie.prototype.revert = function () {
        return __awaiter(this, void 0, void 0, function () {
            var _a;
            return __generator(this, function (_b) {
                switch (_b.label) {
                    case 0:
                        if (!this.isCheckpoint) {
                            throw new Error('trying to revert when not checkpointed');
                        }
                        return [4 /*yield*/, this.lock.wait()];
                    case 1:
                        _b.sent();
                        _a = this;
                        return [4 /*yield*/, this.db.revert()];
                    case 2:
                        _a.root = _b.sent();
                        this.lock.signal();
                        return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Returns a copy of the underlying trie with the interface of CheckpointTrie.
     * @param includeCheckpoints - If true and during a checkpoint, the copy will contain the checkpointing metadata and will use the same scratch as underlying db.
     */
    CheckpointTrie.prototype.copy = function (includeCheckpoints) {
        if (includeCheckpoints === void 0) { includeCheckpoints = true; }
        var db = this.db.copy();
        var trie = new CheckpointTrie(db._leveldb, this.root);
        if (includeCheckpoints && this.isCheckpoint) {
            trie.db.checkpoints = __spreadArray([], __read(this.db.checkpoints), false);
        }
        return trie;
    };
    return CheckpointTrie;
}(baseTrie_1.Trie));
exports.CheckpointTrie = CheckpointTrie;
//# sourceMappingURL=checkpointTrie.js.map