"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
var importsengine_1 = require("./importsengine");
exports.ImportsEngine = importsengine_1.ImportsEngine;
var utils_1 = require("./utils");
exports.findImports = utils_1.findImports;
exports.gatherSources = utils_1.gatherSources;
exports.gatherSourcesAndCanonizeImports = utils_1.gatherSourcesAndCanonizeImports;
const core_1 = require("@resolver-engine/core");
const importparser_1 = require("./parsers/importparser");
const githubresolver_1 = require("./resolvers/githubresolver");
const ipfsresolver_1 = require("./resolvers/ipfsresolver");
const swarmresolver_1 = require("./resolvers/swarmresolver");
exports.resolvers = {
    GithubResolver: githubresolver_1.GithubResolver,
    IPFSResolver: ipfsresolver_1.IPFSResolver,
    SwarmResolver: swarmresolver_1.SwarmResolver,
    UriResolver: core_1.resolvers.UriResolver,
};
exports.parsers = {
    ImportParser: importparser_1.ImportParser,
    UrlParser: core_1.parsers.UrlParser,
};
//# sourceMappingURL=index.js.map