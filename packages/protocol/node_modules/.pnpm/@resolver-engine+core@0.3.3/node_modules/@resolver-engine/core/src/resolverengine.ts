import Debug from "debug";
import { Context } from "./context";
import { SubParser } from "./parsers/subparser";
import { SubResolver } from "./resolvers/subresolver";
import { firstResult } from "./utils";

const debug = Debug("resolverengine:main");

const UNKNOWN_RESOLVER = "unknown";

export interface Options {
  debug?: true;
}

export class ResolverEngine<R> {
  private resolvers: SubResolver[] = [];
  private parsers: Array<SubParser<R>> = [];

  constructor(options?: Options) {
    const opts: Options = { ...options };
    if (opts.debug) {
      Debug.enable("resolverengine:*");
    }
  }

  // Takes a simplified name (URI) and converts into cannonical URL of the location
  public async resolve(uri: string, workingDir?: string): Promise<string> {
    debug(`Resolving "${uri}"`);

    const ctx: Context = {
      cwd: workingDir,
      resolver: UNKNOWN_RESOLVER,
    };

    const result = await firstResult(this.resolvers, resolver => resolver(uri, ctx));

    if (result === null) {
      throw resolverError(uri);
    }

    debug(`Resolved "${uri}" into "${result}"`);

    return result.result;
  }

  public async require(uri: string, workingDir?: string): Promise<R> {
    debug(`Requiring "${uri}"`);

    const ctx: Context = {
      resolver: UNKNOWN_RESOLVER,
      cwd: workingDir,
    };

    const url = await firstResult(this.resolvers, currentResolver => currentResolver(uri, ctx));

    if (url === null) {
      throw resolverError(uri);
    }

    // Through the context we extract information about execution details like the resolver that actually succeeded
    const resolver: any = this.resolvers[url.index];
    const name = typeof resolver === "function" ? resolver.name : resolver.toString();
    ctx.resolver = name;

    const result = await firstResult(this.parsers, parser => parser(url.result, ctx));

    if (result === null) {
      throw parserError(uri);
    }

    return result.result;
  }

  public addResolver(resolver: SubResolver): ResolverEngine<R> {
    this.resolvers.push(resolver);
    return this;
  }

  public addParser(parser: SubParser<R>): ResolverEngine<R> {
    this.parsers.push(parser);
    return this;
  }
}

const resolverError = (uri: string) => new Error(`None of the sub-resolvers resolved "${uri}" location.`);
const parserError = (uri: string) =>
  new Error(`None of the sub-parsers resolved "${uri}" into data. Please confirm your configuration.`);
