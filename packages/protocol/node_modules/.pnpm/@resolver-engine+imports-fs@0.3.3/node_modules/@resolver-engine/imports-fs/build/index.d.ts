export { Context, firstResult, Options, ResolverEngine, SubParser, SubResolver } from "@resolver-engine/core";
export { findImports, gatherSources, gatherSourcesAndCanonizeImports, ImportFile, ImportsEngine, } from "@resolver-engine/imports";
export { ImportsFsEngine } from "./importsfsengine";
import { EthPmResolver } from "./resolvers/ethpmresolver";
export declare const resolvers: {
    EthPmResolver: typeof EthPmResolver;
    UriResolver: typeof import("@resolver-engine/core/build/resolvers").UriResolver;
    BacktrackFsResolver: typeof import("@resolver-engine/fs/build/resolvers/backtrackfsresolver").BacktrackFsResolver;
    FsResolver: typeof import("@resolver-engine/fs/build/resolvers/fsresolver").FsResolver;
    NodeResolver: typeof import("@resolver-engine/fs/build/resolvers/noderesolver").NodeResolver;
    GithubResolver: typeof import("@resolver-engine/imports/build/resolvers/githubresolver").GithubResolver;
    IPFSResolver: typeof import("@resolver-engine/imports/build/resolvers/ipfsresolver").IPFSResolver;
    SwarmResolver: typeof import("@resolver-engine/imports/build/resolvers/swarmresolver").SwarmResolver;
};
export declare const parsers: {
    UrlParser: typeof import("@resolver-engine/core/build/parsers").UrlParser;
    FsParser: typeof import("@resolver-engine/fs/build/parsers/fsparser").FsParser;
    ImportParser: typeof import("@resolver-engine/imports/build/parsers/importparser").ImportParser;
};
