export { Context, firstResult, Options, ResolverEngine, SubParser, SubResolver } from "@resolver-engine/core";
import { parsers as coreParsers, resolvers as coreResolvers } from "@resolver-engine/core";
import { FsParser } from "./parsers/fsparser";
import { BacktrackFsResolver } from "./resolvers/backtrackfsresolver";
import { FsResolver } from "./resolvers/fsresolver";
import { NodeResolver } from "./resolvers/noderesolver";

export const resolvers = {
  BacktrackFsResolver,
  FsResolver,
  NodeResolver,
  UriResolver: coreResolvers.UriResolver,
};

export const parsers = {
  FsParser,
  UrlParser: coreParsers.UrlParser,
};
