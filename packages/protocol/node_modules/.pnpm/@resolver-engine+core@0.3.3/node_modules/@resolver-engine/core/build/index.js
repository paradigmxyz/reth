"use strict";
function __export(m) {
    for (var p in m) if (!exports.hasOwnProperty(p)) exports[p] = m[p];
}
Object.defineProperty(exports, "__esModule", { value: true });
const index_1 = require("./parsers/index");
const index_2 = require("./resolvers/index");
__export(require("./resolverengine"));
__export(require("./utils"));
exports.resolvers = {
    UriResolver: index_2.UriResolver,
};
exports.parsers = {
    UrlParser: index_1.UrlParser,
};
//# sourceMappingURL=index.js.map