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
const debug_1 = __importDefault(require("debug"));
const utils_1 = require("./utils");
const debug = debug_1.default("resolverengine:main");
const UNKNOWN_RESOLVER = "unknown";
class ResolverEngine {
    constructor(options) {
        this.resolvers = [];
        this.parsers = [];
        const opts = Object.assign({}, options);
        if (opts.debug) {
            debug_1.default.enable("resolverengine:*");
        }
    }
    // Takes a simplified name (URI) and converts into cannonical URL of the location
    resolve(uri, workingDir) {
        return __awaiter(this, void 0, void 0, function* () {
            debug(`Resolving "${uri}"`);
            const ctx = {
                cwd: workingDir,
                resolver: UNKNOWN_RESOLVER,
            };
            const result = yield utils_1.firstResult(this.resolvers, resolver => resolver(uri, ctx));
            if (result === null) {
                throw resolverError(uri);
            }
            debug(`Resolved "${uri}" into "${result}"`);
            return result.result;
        });
    }
    require(uri, workingDir) {
        return __awaiter(this, void 0, void 0, function* () {
            debug(`Requiring "${uri}"`);
            const ctx = {
                resolver: UNKNOWN_RESOLVER,
                cwd: workingDir,
            };
            const url = yield utils_1.firstResult(this.resolvers, currentResolver => currentResolver(uri, ctx));
            if (url === null) {
                throw resolverError(uri);
            }
            // Through the context we extract information about execution details like the resolver that actually succeeded
            const resolver = this.resolvers[url.index];
            const name = typeof resolver === "function" ? resolver.name : resolver.toString();
            ctx.resolver = name;
            const result = yield utils_1.firstResult(this.parsers, parser => parser(url.result, ctx));
            if (result === null) {
                throw parserError(uri);
            }
            return result.result;
        });
    }
    addResolver(resolver) {
        this.resolvers.push(resolver);
        return this;
    }
    addParser(parser) {
        this.parsers.push(parser);
        return this;
    }
}
exports.ResolverEngine = ResolverEngine;
const resolverError = (uri) => new Error(`None of the sub-resolvers resolved "${uri}" location.`);
const parserError = (uri) => new Error(`None of the sub-parsers resolved "${uri}" into data. Please confirm your configuration.`);
//# sourceMappingURL=resolverengine.js.map