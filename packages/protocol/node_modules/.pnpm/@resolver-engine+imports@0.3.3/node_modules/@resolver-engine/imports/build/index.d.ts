export { ImportsEngine } from "./importsengine";
export { ImportFile } from "./parsers/importparser";
export { findImports, gatherSources, gatherSourcesAndCanonizeImports } from "./utils";
import { ImportParser } from "./parsers/importparser";
import { GithubResolver } from "./resolvers/githubresolver";
import { IPFSResolver } from "./resolvers/ipfsresolver";
import { SwarmResolver } from "./resolvers/swarmresolver";
export declare const resolvers: {
    GithubResolver: typeof GithubResolver;
    IPFSResolver: typeof IPFSResolver;
    SwarmResolver: typeof SwarmResolver;
    UriResolver: typeof import("@resolver-engine/core/build/resolvers").UriResolver;
};
export declare const parsers: {
    ImportParser: typeof ImportParser;
    UrlParser: typeof import("@resolver-engine/core/build/parsers").UrlParser;
};
