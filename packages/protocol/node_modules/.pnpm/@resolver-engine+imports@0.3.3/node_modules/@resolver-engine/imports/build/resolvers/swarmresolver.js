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
// 1. (hash)
const SWARM_URI = /^bzz-raw:\/\/(\w+)/;
const SWARM_GATEWAY = "https://swarm-gateways.net/bzz-raw:/";
function SwarmResolver() {
    return function swarm(uri, ctx) {
        return __awaiter(this, void 0, void 0, function* () {
            const swarmMatch = uri.match(SWARM_URI);
            if (swarmMatch) {
                const [, hash] = swarmMatch;
                return SWARM_GATEWAY + hash;
            }
            return null;
        });
    };
}
exports.SwarmResolver = SwarmResolver;
//# sourceMappingURL=swarmresolver.js.map