"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const debug_1 = __importDefault(require("debug"));
const is_url_1 = __importDefault(require("is-url"));
const request_1 = __importDefault(require("request"));
const debug = debug_1.default("resolverengine:urlparser");
function UrlParser() {
    return (url) => new Promise((resolve, reject) => {
        if (!is_url_1.default(url)) {
            return resolve(null);
        }
        request_1.default(url, (err, response, body) => {
            if (err) {
                return reject(err);
            }
            if (response.statusCode >= 200 && response.statusCode <= 299) {
                return resolve(body);
            }
            else {
                debug(`Got error: ${response.statusCode} ${response.statusMessage}`);
                return resolve(null);
            }
        });
    });
}
exports.UrlParser = UrlParser;
//# sourceMappingURL=urlparser.js.map