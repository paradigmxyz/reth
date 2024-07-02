import { task, types } from "hardhat/config";
import { NomicLabsHardhatPluginError } from "hardhat/internal/core/errors";
import camelcaseKeys = require("camelcase-keys");
import { registerEnvArgs, registerEvmArgs } from "../common";
import { spawnTest, ForgeTestArgs } from "./test";

registerEvmArgs(registerEnvArgs(task("test")))
  .setDescription("Runs all the test in the project")
  .addFlag("json", "Output test results in JSON format.")
  .addFlag("gasReport", "Print a gas report.")
  .addFlag("allowFailure", "Exit with code 0 even if a test fails.")
  .addOptionalParam(
    "matchTest",
    "Only run test functions matching the specified regex pattern.",
    undefined,
    types.string
  )
  .addOptionalParam(
    "matchContract",
    "Only run tests in contracts matching the specified regex pattern.",
    undefined,
    types.string
  )
  .addOptionalParam(
    "etherscanApiKey",
    "Etherscan API to use",
    undefined,
    types.string
  )
  .setAction(async (args, {}) => {
    const buildArgs = await getCheckedArgs(args);
    await spawnTest(buildArgs);
  });

async function getCheckedArgs(args: any): Promise<ForgeTestArgs> {
  // Get and initialize option validator
  const { default: buildArgsSchema } = await import("../build/build-ti");
  const { default: envArgsSchema } = await import("../common/env-ti");
  const { default: evmArgsSchema } = await import("../common/evm-ti");
  const { default: compilerArgsSchema } = await import("../common/compiler-ti");
  const { default: projectPathsSchema } = await import(
    "../common/projectpaths-ti"
  );
  const { createCheckers } = await import("ts-interface-checker");
  const { ForgeBuildArgsTi } = createCheckers(
    buildArgsSchema,
    envArgsSchema,
    evmArgsSchema,
    compilerArgsSchema,
    projectPathsSchema
  );
  const uncheckedBuildArgs = camelcaseKeys(args);
  // Validate all options against the validator
  try {
    ForgeBuildArgsTi.check(uncheckedBuildArgs);
  } catch (e: any) {
    throw new NomicLabsHardhatPluginError(
      "@foundry-rs/hardhat-forge",
      `Forge build config is invalid: ${e.message}`,
      e
    );
  }
  return uncheckedBuildArgs as ForgeTestArgs;
}
