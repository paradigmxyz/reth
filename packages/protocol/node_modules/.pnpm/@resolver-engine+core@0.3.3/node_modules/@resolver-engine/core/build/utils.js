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
function firstResult(things, check, ctx) {
    return __awaiter(this, void 0, void 0, function* () {
        for (let index = 0; index < things.length; index++) {
            const result = yield check(things[index]);
            if (result) {
                return { result, index };
            }
        }
        return null;
    });
}
exports.firstResult = firstResult;
//# sourceMappingURL=utils.js.map