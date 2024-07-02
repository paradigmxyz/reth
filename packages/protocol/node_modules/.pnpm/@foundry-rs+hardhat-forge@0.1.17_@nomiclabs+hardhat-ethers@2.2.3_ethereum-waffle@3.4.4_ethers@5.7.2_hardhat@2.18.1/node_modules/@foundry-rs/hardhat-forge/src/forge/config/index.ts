// bindings for forge config

import * as foundryup from "@foundry-rs/easy-foundryup";
import { spawn, spawnSync } from "child_process";
import { task } from "hardhat/config";
import { FoundryConfig } from "./config";

task("forge:config")
  .setDescription("Returns the config of the current project")
  .setAction(async () => {
    return spawnConfig();
  });

/** *
 * Invokes `forge config` and returns the current config of the current project
 */
export async function spawnConfig(): Promise<FoundryConfig> {
  const args = ["config", "--json"];
  const forgeCmd = await foundryup.getForgeCommand();
  return new Promise((resolve) => {
    const process = spawn(forgeCmd, args);
    let config = "";
    process.stdout.on("data", (data) => {
      config += data.toString();
    });

    process.on("exit", (_code) => {
      resolve(JSON.parse(config) as FoundryConfig);
    });
  });
}

/** *
 * Invokes `forge config` and returns the current config of the current project
 */
export function spawnConfigSync(): FoundryConfig {
  const args = ["config", "--json"];
  const forgeCmd = foundryup.getForgeCommandSync();
  const res = spawnSync(forgeCmd, args);
  return JSON.parse(res.stdout.toString()) as FoundryConfig;
}
