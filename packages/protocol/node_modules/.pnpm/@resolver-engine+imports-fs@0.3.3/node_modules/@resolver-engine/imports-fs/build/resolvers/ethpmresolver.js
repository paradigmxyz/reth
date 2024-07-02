"use strict";
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : new P(function (resolve) { resolve(result.value); }).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const fs_1 = require("@resolver-engine/fs");
const debug_1 = __importDefault(require("debug"));
const debug = debug_1.default("resolverengine:ethpmresolver");
// 1st group - package name
// 2nd group - contract path
const FILE_LOCATION_REGEX = /^([^/]+)\/(.+)$/;
const prefixTruffle = "installed_contracts";
const prefix0x = "contracts";
function EthPmResolver() {
    const backtrackT = fs_1.resolvers.BacktrackFsResolver(prefixTruffle);
    const backtrack0x = fs_1.resolvers.BacktrackFsResolver(prefix0x);
    return function ethpm(what, ctx) {
        return __awaiter(this, void 0, void 0, function* () {
            const fileMatch = what.match(FILE_LOCATION_REGEX);
            if (!fileMatch) {
                return null;
            }
            let result = yield backtrackT(what, ctx);
            if (result) {
                debug("Resolved %s to %s", what, result);
                return result;
            }
            result = yield backtrack0x(what, ctx);
            if (result) {
                debug("Resolved %s to %s", what, result);
                return result;
            }
            return null;
        });
    };
}
exports.EthPmResolver = EthPmResolver;
//# sourceMappingURL=ethpmresolver.js.map