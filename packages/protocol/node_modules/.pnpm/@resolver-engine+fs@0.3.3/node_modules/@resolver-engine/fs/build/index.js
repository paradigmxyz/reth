"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
var core_1 = require("@resolver-engine/core");
exports.firstResult = core_1.firstResult;
exports.ResolverEngine = core_1.ResolverEngine;
const core_2 = require("@resolver-engine/core");
const fsparser_1 = require("./parsers/fsparser");
const backtrackfsresolver_1 = require("./resolvers/backtrackfsresolver");
const fsresolver_1 = require("./resolvers/fsresolver");
const noderesolver_1 = require("./resolvers/noderesolver");
exports.resolvers = {
    BacktrackFsResolver: backtrackfsresolver_1.BacktrackFsResolver,
    FsResolver: fsresolver_1.FsResolver,
    NodeResolver: noderesolver_1.NodeResolver,
    UriResolver: core_2.resolvers.UriResolver,
};
exports.parsers = {
    FsParser: fsparser_1.FsParser,
    UrlParser: core_2.parsers.UrlParser,
};
//# sourceMappingURL=index.js.map