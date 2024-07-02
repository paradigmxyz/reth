export { Context, firstResult, Options, ResolverEngine, SubParser, SubResolver } from "@resolver-engine/core";
export {
  findImports,
  gatherSources,
  gatherSourcesAndCanonizeImports,
  ImportFile,
  ImportsEngine,
} from "@resolver-engine/imports";
export { ImportsFsEngine } from "./importsfsengine";
import { parsers as coreParsers, resolvers as coreResolvers } from "@resolver-engine/core";
import { parsers as fsParsers, resolvers as fsResolvers } from "@resolver-engine/fs";
import { parsers as importsParsers, resolvers as importsResolvers } from "@resolver-engine/imports";
import { EthPmResolver } from "./resolvers/ethpmresolver";

// TODO(cymerrad)
// object destructuring doesn't work in this case
// i.e.: ...fsResolvers, ...importsResolvers
// generated import paths in *.d.ts point to invalid files
// this is a more laborious way of achieving the same goal

export const resolvers = {
  EthPmResolver,
  UriResolver: coreResolvers.UriResolver,
  BacktrackFsResolver: fsResolvers.BacktrackFsResolver,
  FsResolver: fsResolvers.FsResolver,
  NodeResolver: fsResolvers.NodeResolver,
  GithubResolver: importsResolvers.GithubResolver,
  IPFSResolver: importsResolvers.IPFSResolver,
  SwarmResolver: importsResolvers.SwarmResolver,
};

export const parsers = {
  UrlParser: coreParsers.UrlParser,
  FsParser: fsParsers.FsParser,
  ImportParser: importsParsers.ImportParser,
};
