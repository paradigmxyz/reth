import { UrlParser } from "./parsers/index";
import { UriResolver } from "./resolvers/index";
export * from "./context";
export { SubParser } from "./parsers/index";
export * from "./resolverengine";
export { SubResolver } from "./resolvers/index";
export * from "./utils";
export declare const resolvers: {
    UriResolver: typeof UriResolver;
};
export declare const parsers: {
    UrlParser: typeof UrlParser;
};
