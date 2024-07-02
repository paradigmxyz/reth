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
var __importStar = (this && this.__importStar) || function (mod) {
    if (mod && mod.__esModule) return mod;
    var result = {};
    if (mod != null) for (var k in mod) if (Object.hasOwnProperty.call(mod, k)) result[k] = mod[k];
    result["default"] = mod;
    return result;
};
Object.defineProperty(exports, "__esModule", { value: true });
const debug_1 = __importDefault(require("debug"));
const fs = __importStar(require("fs"));
const util_1 = require("util");
const readFileAsync = util_1.promisify(fs.readFile);
const debug = debug_1.default("resolverengine:fsparser");
function FsParser() {
    return (path) => __awaiter(this, void 0, void 0, function* () {
        try {
            return (yield readFileAsync(path, "utf-8")).toString();
        }
        catch (e) {
            debug(`Error returned when trying to parse "${path}", returning null`, e);
            return null;
        }
    });
}
exports.FsParser = FsParser;
//# sourceMappingURL=fsparser.js.map