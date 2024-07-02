"use strict";
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : new P(function (resolve) { resolve(result.value); }).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
Object.defineProperty(exports, "__esModule", { value: true });
// 1. (root / path to resource)
const IPFS_URI = /^ipfs:\/\/(.+)$/;
function IPFSResolver() {
    return function ipfs(uri, ctx) {
        return __awaiter(this, void 0, void 0, function* () {
            const ipfsMatch = uri.match(IPFS_URI);
            if (ipfsMatch) {
                const [, resourcePath] = ipfsMatch;
                const url = "https://gateway.ipfs.io/ipfs/" + resourcePath;
                return url;
            }
            return null;
        });
    };
}
exports.IPFSResolver = IPFSResolver;
//# sourceMappingURL=ipfsresolver.js.map