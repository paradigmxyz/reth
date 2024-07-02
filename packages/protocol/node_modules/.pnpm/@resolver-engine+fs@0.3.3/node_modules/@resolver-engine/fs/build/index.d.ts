export { Context, firstResult, Options, ResolverEngine, SubParser, SubResolver } from "@resolver-engine/core";
import { FsParser } from "./parsers/fsparser";
import { BacktrackFsResolver } from "./resolvers/backtrackfsresolver";
import { FsResolver } from "./resolvers/fsresolver";
import { NodeResolver } from "./resolvers/noderesolver";
export declare const resolvers: {
    BacktrackFsResolver: typeof BacktrackFsResolver;
    FsResolver: typeof FsResolver;
    NodeResolver: typeof NodeResolver;
    UriResolver: typeof import("@resolver-engine/core/build/resolvers").UriResolver;
};
export declare const parsers: {
    FsParser: typeof FsParser;
    UrlParser: typeof import("@resolver-engine/core/build/parsers").UrlParser;
};
