import { parsers, ResolverEngine, resolvers } from "@resolver-engine/core";
import { ImportFile, ImportParser } from "./parsers/importparser";
import { GithubResolver } from "./resolvers/githubresolver";
import { IPFSResolver } from "./resolvers/ipfsresolver";
import { SwarmResolver } from "./resolvers/swarmresolver";

export function ImportsEngine(): ResolverEngine<ImportFile> {
  return new ResolverEngine<ImportFile>()
    .addResolver(GithubResolver())
    .addResolver(SwarmResolver())
    .addResolver(IPFSResolver())
    .addResolver(resolvers.UriResolver())
    .addParser(ImportParser([parsers.UrlParser()]));
}
