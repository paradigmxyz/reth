import { SubParser } from "./parsers/subparser";
import { SubResolver } from "./resolvers/subresolver";
export interface Options {
    debug?: true;
}
export declare class ResolverEngine<R> {
    private resolvers;
    private parsers;
    constructor(options?: Options);
    resolve(uri: string, workingDir?: string): Promise<string>;
    require(uri: string, workingDir?: string): Promise<R>;
    addResolver(resolver: SubResolver): ResolverEngine<R>;
    addParser(parser: SubParser<R>): ResolverEngine<R>;
}
