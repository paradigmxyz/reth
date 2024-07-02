// bindings for forge test
import { spawn as spawn } from "child_process";
import * as foundryup from "@foundry-rs/easy-foundryup";
import { buildArgs, ForgeBuildArgs } from "../build/build";
import { envArgs, evmArgs, ForgeEvmArgs } from "../common";

/**
 * Mirrors the `forge test` arguments
 */
export interface ForgeTestArgs extends ForgeBuildArgs, ForgeEvmArgs {
  json?: boolean;
  gasReport?: boolean;
  allowFailure?: boolean;
  etherscanApiKey?: string;
  matchTest?: string;
  matchContract?: string;
}

/** *
 * Invokes `forge build`
 * @param opts The arguments to pass to `forge build`
 */
export async function spawnTest(opts: ForgeTestArgs): Promise<boolean> {
  const args = ["test", ...testArgs(opts)];
  const forgeCmd = await foundryup.getForgeCommand();
  return new Promise((resolve) => {
    const process = spawn(forgeCmd, args, {
      stdio: "inherit",
    });
    process.on("exit", (code) => {
      resolve(code === 0);
    });
  });
}

/**
 * Converts the `args` object into a list of arguments for the `forge test` command
 * @param args
 */
export function testArgs(args: ForgeTestArgs): string[] {
  const allArgs: string[] = [];
  const etherscanApiKey = args.etherscanApiKey ?? "";
  if (etherscanApiKey) {
    allArgs.push("--etherscan-api-key", etherscanApiKey);
  }

  if (args.json ?? false) {
    allArgs.push("--json");
  }

  if (args.allowFailure ?? false) {
    allArgs.push("--allow-failure");
  }

  if (args.gasReport ?? false) {
    allArgs.push("--gas-report");
  }

  const matchTest = args.matchTest ?? "";
  if (matchTest) {
    allArgs.push("--match-test", matchTest);
  }

  const matchContract = args.matchContract ?? "";
  if (matchContract) {
    allArgs.push("--match-contract", matchContract);
  }

  allArgs.push(...buildArgs(args));
  allArgs.push(...evmArgs(args));
  allArgs.push(...envArgs(args));

  return allArgs;
}
