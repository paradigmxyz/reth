"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
var core_1 = require("@resolver-engine/core");
exports.firstResult = core_1.firstResult;
exports.ResolverEngine = core_1.ResolverEngine;
var imports_1 = require("@resolver-engine/imports");
exports.findImports = imports_1.findImports;
exports.gatherSources = imports_1.gatherSources;
exports.gatherSourcesAndCanonizeImports = imports_1.gatherSourcesAndCanonizeImports;
exports.ImportsEngine = imports_1.ImportsEngine;
var importsfsengine_1 = require("./importsfsengine");
exports.ImportsFsEngine = importsfsengine_1.ImportsFsEngine;
const core_2 = require("@resolver-engine/core");
const fs_1 = require("@resolver-engine/fs");
const imports_2 = require("@resolver-engine/imports");
const ethpmresolver_1 = require("./resolvers/ethpmresolver");
// TODO(cymerrad)
// object destructuring doesn't work in this case
// i.e.: ...fsResolvers, ...importsResolvers
// generated import paths in *.d.ts point to invalid files
// this is a more laborious way of achieving the same goal
exports.resolvers = {
    EthPmResolver: ethpmresolver_1.EthPmResolver,
    UriResolver: core_2.resolvers.UriResolver,
    BacktrackFsResolver: fs_1.resolvers.BacktrackFsResolver,
    FsResolver: fs_1.resolvers.FsResolver,
    NodeResolver: fs_1.resolvers.NodeResolver,
    GithubResolver: imports_2.resolvers.GithubResolver,
    IPFSResolver: imports_2.resolvers.IPFSResolver,
    SwarmResolver: imports_2.resolvers.SwarmResolver,
};
exports.parsers = {
    UrlParser: core_2.parsers.UrlParser,
    FsParser: fs_1.parsers.FsParser,
    ImportParser: imports_2.parsers.ImportParser,
};
//# sourceMappingURL=index.js.map