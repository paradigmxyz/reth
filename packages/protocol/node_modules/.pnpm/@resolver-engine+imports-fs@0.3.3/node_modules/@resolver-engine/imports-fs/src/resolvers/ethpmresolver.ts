import { Context, SubResolver } from "@resolver-engine/core";
import { resolvers } from "@resolver-engine/fs";
import Debug from "debug";
const debug = Debug("resolverengine:ethpmresolver");

// 1st group - package name
// 2nd group - contract path
const FILE_LOCATION_REGEX = /^([^/]+)\/(.+)$/;

const prefixTruffle = "installed_contracts";
const prefix0x = "contracts";

export function EthPmResolver(): SubResolver {
  const backtrackT = resolvers.BacktrackFsResolver(prefixTruffle);
  const backtrack0x = resolvers.BacktrackFsResolver(prefix0x);

  return async function ethpm(what: string, ctx: Context): Promise<string | null> {
    const fileMatch = what.match(FILE_LOCATION_REGEX);
    if (!fileMatch) {
      return null;
    }

    let result = await backtrackT(what, ctx);
    if (result) {
      debug("Resolved %s to %s", what, result);
      return result;
    }

    result = await backtrack0x(what, ctx);
    if (result) {
      debug("Resolved %s to %s", what, result);
      return result;
    }

    return null;
  };
}
