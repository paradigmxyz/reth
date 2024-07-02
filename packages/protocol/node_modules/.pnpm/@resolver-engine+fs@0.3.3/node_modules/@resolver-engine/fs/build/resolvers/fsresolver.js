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
const fsSys = __importStar(require("fs"));
const pathSys = __importStar(require("path"));
const statAsync = (path) => new Promise((resolve, reject) => {
    fsSys.stat(path, (err, stats) => {
        if (err) {
            reject(err);
        }
        resolve(stats);
    });
});
const NO_FILE = "ENOENT";
function FsResolver() {
    return function fs(resolvePath, ctx) {
        return __awaiter(this, void 0, void 0, function* () {
            const cwd = ctx.cwd || process.cwd();
            let myPath;
            if (!pathSys.isAbsolute(resolvePath)) {
                myPath = pathSys.join(cwd, resolvePath);
            }
            else {
                myPath = resolvePath;
            }
            try {
                const stats = yield statAsync(myPath);
                return stats.isFile() ? myPath : null;
            }
            catch (e) {
                if (e.code === NO_FILE) {
                    return null;
                }
                throw e;
            }
        });
    };
}
exports.FsResolver = FsResolver;
//# sourceMappingURL=fsresolver.js.map