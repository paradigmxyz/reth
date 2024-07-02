import { UrlParser } from "./parsers/index";
import { UriResolver } from "./resolvers/index";

// exporting interfaces, types and classes
// packing all the resolvers and parsers inside namespaces

export * from "./context";
export { SubParser } from "./parsers/index";
export * from "./resolverengine";
export { SubResolver } from "./resolvers/index";
export * from "./utils";

export const resolvers = {
  UriResolver,
};

export const parsers = {
  UrlParser,
};
