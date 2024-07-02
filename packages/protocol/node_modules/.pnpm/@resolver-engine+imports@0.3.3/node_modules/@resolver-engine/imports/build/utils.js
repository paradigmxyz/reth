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
Object.defineProperty(exports, "__esModule", { value: true });
const path_1 = __importDefault(require("path"));
const url_1 = __importDefault(require("url"));
function findImports(data) {
    const result = [];
    // regex below matches all possible import statements, namely:
    // - import "somefile";
    // - import "somefile" as something;
    // - import something from "somefile"
    // (double that for single quotes)
    // and captures file names
    const regex = /import\s+(?:(?:"([^;]*)"|'([^;]*)')(?:;|\s+as\s+[^;]*;)|.+from\s+(?:"(.*)"|'(.*)');)/g;
    let match;
    // tslint:disable-next-line:no-conditional-assignment
    while ((match = regex.exec(data.source))) {
        for (let i = 1; i < match.length; i++) {
            if (match[i] !== undefined) {
                result.push(match[i]);
                break;
            }
        }
    }
    return result;
}
exports.findImports = findImports;
/**
 * This function accepts root files to be searched for and resolves the sources, finds the imports in each source and traverses the whole dependency tree gathering absolute and uri paths
 * @param roots
 * @param workingDir
 * @param resolver
 */
function gatherDepenencyTree(roots, workingDir, resolver) {
    return __awaiter(this, void 0, void 0, function* () {
        const result = [];
        const alreadyImported = new Set();
        /**
         * This function traverses the depedency tree and calculates absolute paths for each import on the way storing each file in in a global array
         * @param file File in a depedency that should now be traversed
         * @returns An absolute path for the requested file
         */
        function dfs(file) {
            return __awaiter(this, void 0, void 0, function* () {
                const url = yield resolver.resolve(file.uri, file.searchCwd);
                if (alreadyImported.has(url)) {
                    return url;
                }
                const resolvedFile = yield resolver.require(file.uri, file.searchCwd);
                alreadyImported.add(url);
                const foundImportURIs = findImports(resolvedFile);
                const fileNode = Object.assign({ uri: file.uri, imports: [] }, resolvedFile);
                const resolvedCwd = path_1.default.dirname(url);
                for (const importUri of foundImportURIs) {
                    const importUrl = yield dfs({ searchCwd: resolvedCwd, uri: importUri });
                    fileNode.imports.push({ uri: importUri, url: importUrl });
                }
                result.push(fileNode);
                return resolvedFile.url;
            });
        }
        yield Promise.all(roots.map(what => dfs({ searchCwd: workingDir, uri: what })));
        return result;
    });
}
function stripNodes(nodes) {
    return nodes.map(node => {
        return { url: node.url, source: node.source, provider: node.provider };
    });
}
/**
 * Starts with roots and traverses the whole depedency tree of imports, returning an array of sources
 * @param roots
 * @param workingDir What's the starting working dir for resolving relative imports in roots
 * @param resolver
 */
function gatherSources(roots, workingDir, resolver) {
    return __awaiter(this, void 0, void 0, function* () {
        const result = [];
        const queue = [];
        const alreadyImported = new Set();
        if (workingDir !== "") {
            workingDir += "/";
        }
        const absoluteRoots = roots.map(what => url_1.default.resolve(workingDir, what));
        for (const absWhat of absoluteRoots) {
            queue.push({ cwd: workingDir, file: absWhat, relativeTo: workingDir });
            alreadyImported.add(absWhat);
        }
        while (queue.length > 0) {
            const fileData = queue.shift();
            const resolvedFile = yield resolver.require(fileData.file, fileData.cwd);
            const foundImports = findImports(resolvedFile);
            // if imported path starts with '.' we assume it's relative and return it's
            // path relative to resolved name of the file that imported it
            // if not - return the same name it was imported with
            let relativePath;
            if (fileData.file[0] === ".") {
                relativePath = url_1.default.resolve(fileData.relativeTo, fileData.file);
                result.push({ url: relativePath, source: resolvedFile.source, provider: resolvedFile.provider });
            }
            else {
                relativePath = fileData.file;
                result.push({ url: relativePath, source: resolvedFile.source, provider: resolvedFile.provider });
            }
            const fileParentDir = path_1.default.dirname(resolvedFile.url);
            for (const foundImport of foundImports) {
                let importName;
                if (foundImport[0] === ".") {
                    importName = url_1.default.resolve(relativePath, foundImport);
                }
                else {
                    importName = foundImport;
                }
                if (!alreadyImported.has(importName)) {
                    alreadyImported.add(importName);
                    queue.push({ cwd: fileParentDir, file: foundImport, relativeTo: relativePath });
                }
            }
        }
        return result;
    });
}
exports.gatherSources = gatherSources;
/**
 * This function gathers sources and **REWRITES IMPORTS** inside the source files into resolved, absolute paths instead of using shortcut forms
 * Because the remapping api in solc is not compatible with multiple existing projects and frameworks, changing relative paths to absolute paths
 * makes us avoid any need for finding imports after starting the solc compilation
 * @param roots
 * @param workingDir What's the starting working dir for resolving relative imports in roots
 * @param resolver
 */
function gatherSourcesAndCanonizeImports(roots, workingDir, resolver) {
    return __awaiter(this, void 0, void 0, function* () {
        function canonizeFile(file) {
            file.imports.forEach(i => (file.source = file.source.replace(i.uri, i.url)));
        }
        const sources = yield gatherDepenencyTree(roots, workingDir, resolver);
        sources.forEach(canonizeFile);
        return stripNodes(sources);
    });
}
exports.gatherSourcesAndCanonizeImports = gatherSourcesAndCanonizeImports;
//# sourceMappingURL=utils.js.map