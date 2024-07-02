import { Context, SubResolver } from "@resolver-engine/core";

// 1. (hash)
const SWARM_URI = /^bzz-raw:\/\/(\w+)/;
const SWARM_GATEWAY = "https://swarm-gateways.net/bzz-raw:/";

export function SwarmResolver(): SubResolver {
  return async function swarm(uri: string, ctx: Context): Promise<string | null> {
    const swarmMatch = uri.match(SWARM_URI);
    if (swarmMatch) {
      const [, hash] = swarmMatch;
      return SWARM_GATEWAY + hash;
    }

    return null;
  };
}
