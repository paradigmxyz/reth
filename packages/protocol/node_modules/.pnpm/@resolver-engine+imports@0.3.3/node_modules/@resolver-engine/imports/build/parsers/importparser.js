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
const core_1 = require("@resolver-engine/core");
const debug_1 = __importDefault(require("debug"));
const debug = debug_1.default("resolverengine:importparser");
function ImportParser(sourceParsers) {
    return (url, ctx) => __awaiter(this, void 0, void 0, function* () {
        const source = yield core_1.firstResult(sourceParsers, parser => parser(url, ctx));
        if (!source) {
            debug(`Can't find source for ${url}`);
            return null;
        }
        const provider = ctx.resolver;
        return {
            url,
            source: source.result,
            provider,
        };
    });
}
exports.ImportParser = ImportParser;
//# sourceMappingURL=importparser.js.map