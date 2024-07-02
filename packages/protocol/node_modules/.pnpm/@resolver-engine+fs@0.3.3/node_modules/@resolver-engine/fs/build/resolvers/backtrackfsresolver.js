"use strict";
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : new P(function (resolve) { resolve(result.value); }).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __importStar = (this && this.__importStar) || function (mod) {
    if (mod && mod.__esModule) return mod;
    var result = {};
    if (mod != null) for (var k in mod) if (Object.hasOwnProperty.call(mod, k)) result[k] = mod[k];
    result["default"] = mod;
    return result;
};
Object.defineProperty(exports, "__esModule", { value: true });
const path = __importStar(require("path"));
const fsresolver_1 = require("./fsresolver");
function BacktrackFsResolver(pathPrefix = "") {
    const fsResolver = fsresolver_1.FsResolver();
    return function backtrack(resolvePath, ctx) {
        return __awaiter(this, void 0, void 0, function* () {
            if (path.isAbsolute(resolvePath)) {
                return null;
            }
            const cwd = ctx.cwd || process.cwd();
            let previous = path.resolve(cwd, "./");
            let current = previous;
            do {
                const result = yield fsResolver(path.join(current, pathPrefix, resolvePath), ctx);
                if (result) {
                    return result;
                }
                previous = current;
                current = path.join(current, "..");
            } while (current !== previous); // Reached root
            return null;
        });
    };
}
exports.BacktrackFsResolver = BacktrackFsResolver;
//# sourceMappingURL=backtrackfsresolver.js.map