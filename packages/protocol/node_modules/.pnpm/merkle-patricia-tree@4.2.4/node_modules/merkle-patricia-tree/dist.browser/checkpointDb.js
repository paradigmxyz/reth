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
exports.CheckpointDB = void 0;
var db_1 = require("./db");
/**
 * DB is a thin wrapper around the underlying levelup db,
 * which validates inputs and sets encoding type.
 */
var CheckpointDB = /** @class */ (function (_super) {
    __extends(CheckpointDB, _super);
    /**
     * Initialize a DB instance. If `leveldb` is not provided, DB
     * defaults to an [in-memory store](https://github.com/Level/memdown).
     * @param leveldb - An abstract-leveldown compliant store
     */
    function CheckpointDB(leveldb) {
        var _this = _super.call(this, leveldb) || this;
        // Roots of trie at the moment of checkpoint
        _this.checkpoints = [];
        return _this;
    }
    Object.defineProperty(CheckpointDB.prototype, "isCheckpoint", {
        /**
         * Is the DB during a checkpoint phase?
         */
        get: function () {
            return this.checkpoints.length > 0;
        },
        enumerable: false,
        configurable: true
    });
    /**
     * Adds a new checkpoint to the stack
     * @param root
     */
    CheckpointDB.prototype.checkpoint = function (root) {
        this.checkpoints.push({ keyValueMap: new Map(), root: root });
    };
    /**
     * Commits the latest checkpoint
     */
    CheckpointDB.prototype.commit = function () {
        return __awaiter(this, void 0, void 0, function () {
            var keyValueMap, batchOp_1, currentKeyValueMap_1;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        keyValueMap = this.checkpoints.pop().keyValueMap;
                        if (!!this.isCheckpoint) return [3 /*break*/, 2];
                        batchOp_1 = [];
                        keyValueMap.forEach(function (value, key) {
                            if (value === null) {
                                batchOp_1.push({
                                    type: 'del',
                                    key: Buffer.from(key, 'binary'),
                                });
                            }
                            else {
                                batchOp_1.push({
                                    type: 'put',
                                    key: Buffer.from(key, 'binary'),
                                    value: value,
                                });
                            }
                        });
                        return [4 /*yield*/, this.batch(batchOp_1)];
                    case 1:
                        _a.sent();
                        return [3 /*break*/, 3];
                    case 2:
                        currentKeyValueMap_1 = this.checkpoints[this.checkpoints.length - 1].keyValueMap;
                        keyValueMap.forEach(function (value, key) { return currentKeyValueMap_1.set(key, value); });
                        _a.label = 3;
                    case 3: return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Reverts the latest checkpoint
     */
    CheckpointDB.prototype.revert = function () {
        return __awaiter(this, void 0, void 0, function () {
            var root;
            return __generator(this, function (_a) {
                root = this.checkpoints.pop().root;
                return [2 /*return*/, root];
            });
        });
    };
    /**
     * Retrieves a raw value from leveldb.
     * @param key
     * @returns A Promise that resolves to `Buffer` if a value is found or `null` if no value is found.
     */
    CheckpointDB.prototype.get = function (key) {
        return __awaiter(this, void 0, void 0, function () {
            var index, value_1, value;
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        // Lookup the value in our cache. We return the latest checkpointed value (which should be the value on disk)
                        for (index = this.checkpoints.length - 1; index >= 0; index--) {
                            value_1 = this.checkpoints[index].keyValueMap.get(key.toString('binary'));
                            if (value_1 !== undefined) {
                                return [2 /*return*/, value_1];
                            }
                        }
                        return [4 /*yield*/, _super.prototype.get.call(this, key)];
                    case 1:
                        value = _a.sent();
                        if (this.isCheckpoint) {
                            // Since we are a checkpoint, put this value in cache, so future `get` calls will not look the key up again from disk.
                            this.checkpoints[this.checkpoints.length - 1].keyValueMap.set(key.toString('binary'), value);
                        }
                        return [2 /*return*/, value];
                }
            });
        });
    };
    /**
     * Writes a value directly to leveldb.
     * @param key The key as a `Buffer`
     * @param value The value to be stored
     */
    CheckpointDB.prototype.put = function (key, val) {
        return __awaiter(this, void 0, void 0, function () {
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        if (!this.isCheckpoint) return [3 /*break*/, 1];
                        // put value in cache
                        this.checkpoints[this.checkpoints.length - 1].keyValueMap.set(key.toString('binary'), val);
                        return [3 /*break*/, 3];
                    case 1: return [4 /*yield*/, _super.prototype.put.call(this, key, val)];
                    case 2:
                        _a.sent();
                        _a.label = 3;
                    case 3: return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Removes a raw value in the underlying leveldb.
     * @param keys
     */
    CheckpointDB.prototype.del = function (key) {
        return __awaiter(this, void 0, void 0, function () {
            return __generator(this, function (_a) {
                switch (_a.label) {
                    case 0:
                        if (!this.isCheckpoint) return [3 /*break*/, 1];
                        // delete the value in the current cache
                        this.checkpoints[this.checkpoints.length - 1].keyValueMap.set(key.toString('binary'), null);
                        return [3 /*break*/, 3];
                    case 1: 
                    // delete the value on disk
                    return [4 /*yield*/, this._leveldb.del(key, db_1.ENCODING_OPTS)];
                    case 2:
                        // delete the value on disk
                        _a.sent();
                        _a.label = 3;
                    case 3: return [2 /*return*/];
                }
            });
        });
    };
    /**
     * Performs a batch operation on db.
     * @param opStack A stack of levelup operations
     */
    CheckpointDB.prototype.batch = function (opStack) {
        return __awaiter(this, void 0, void 0, function () {
            var opStack_1, opStack_1_1, op, e_1_1;
            var e_1, _a;
            return __generator(this, function (_b) {
                switch (_b.label) {
                    case 0:
                        if (!this.isCheckpoint) return [3 /*break*/, 11];
                        _b.label = 1;
                    case 1:
                        _b.trys.push([1, 8, 9, 10]);
                        opStack_1 = __values(opStack), opStack_1_1 = opStack_1.next();
                        _b.label = 2;
                    case 2:
                        if (!!opStack_1_1.done) return [3 /*break*/, 7];
                        op = opStack_1_1.value;
                        if (!(op.type === 'put')) return [3 /*break*/, 4];
                        return [4 /*yield*/, this.put(op.key, op.value)];
                    case 3:
                        _b.sent();
                        return [3 /*break*/, 6];
                    case 4:
                        if (!(op.type === 'del')) return [3 /*break*/, 6];
                        return [4 /*yield*/, this.del(op.key)];
                    case 5:
                        _b.sent();
                        _b.label = 6;
                    case 6:
                        opStack_1_1 = opStack_1.next();
                        return [3 /*break*/, 2];
                    case 7: return [3 /*break*/, 10];
                    case 8:
                        e_1_1 = _b.sent();
                        e_1 = { error: e_1_1 };
                        return [3 /*break*/, 10];
                    case 9:
                        try {
                            if (opStack_1_1 && !opStack_1_1.done && (_a = opStack_1.return)) _a.call(opStack_1);
                        }
                        finally { if (e_1) throw e_1.error; }
                        return [7 /*endfinally*/];
                    case 10: return [3 /*break*/, 13];
                    case 11: return [4 /*yield*/, _super.prototype.batch.call(this, opStack)];
                    case 12:
                        _b.sent();
                        _b.label = 13;
                    case 13: return [2 /*return*/];
                }
            });
        });
    };
    return CheckpointDB;
}(db_1.DB));
exports.CheckpointDB = CheckpointDB;
//# sourceMappingURL=checkpointDb.js.map