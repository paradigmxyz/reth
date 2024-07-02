import { Context, SubResolver } from "@resolver-engine/core";
import { BacktrackFsResolver } from "./backtrackfsresolver";

export function NodeResolver(): SubResolver {
  const backtrack = BacktrackFsResolver("node_modules");

  return async function node(what: string, ctx: Context): Promise<string | null> {
    return backtrack(what, ctx);
  };
}
