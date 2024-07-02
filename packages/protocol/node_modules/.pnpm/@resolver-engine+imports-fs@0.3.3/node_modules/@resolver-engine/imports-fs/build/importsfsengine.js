"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const fs_1 = require("@resolver-engine/fs");
const imports_1 = require("@resolver-engine/imports");
const ethpmresolver_1 = require("./resolvers/ethpmresolver");
function ImportsFsEngine() {
    return imports_1.ImportsEngine()
        .addResolver(fs_1.resolvers.FsResolver())
        .addResolver(fs_1.resolvers.NodeResolver())
        .addResolver(ethpmresolver_1.EthPmResolver())
        .addParser(imports_1.parsers.ImportParser([fs_1.parsers.FsParser()]));
}
exports.ImportsFsEngine = ImportsFsEngine;
//# sourceMappingURL=importsfsengine.js.map