"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const core_1 = require("@resolver-engine/core");
const importparser_1 = require("./parsers/importparser");
const githubresolver_1 = require("./resolvers/githubresolver");
const ipfsresolver_1 = require("./resolvers/ipfsresolver");
const swarmresolver_1 = require("./resolvers/swarmresolver");
function ImportsEngine() {
    return new core_1.ResolverEngine()
        .addResolver(githubresolver_1.GithubResolver())
        .addResolver(swarmresolver_1.SwarmResolver())
        .addResolver(ipfsresolver_1.IPFSResolver())
        .addResolver(core_1.resolvers.UriResolver())
        .addParser(importparser_1.ImportParser([core_1.parsers.UrlParser()]));
}
exports.ImportsEngine = ImportsEngine;
//# sourceMappingURL=importsengine.js.map