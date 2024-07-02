import isUrl from "is-url";
import { Context } from "..";
import { SubResolver } from "./subresolver";

export function UriResolver(): SubResolver {
  return async function http(uri: string, ctx: Context): Promise<string | null> {
    if (!isUrl(uri)) {
      return null;
    }
    return new URL(uri).href;
  };
}
