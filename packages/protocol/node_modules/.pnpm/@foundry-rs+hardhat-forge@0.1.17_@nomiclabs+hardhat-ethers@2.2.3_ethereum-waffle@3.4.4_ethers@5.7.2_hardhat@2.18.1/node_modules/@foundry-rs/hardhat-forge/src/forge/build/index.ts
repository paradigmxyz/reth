import { task } from "hardhat/config";
import camelcaseKeys = require("camelcase-keys");
import { NomicLabsHardhatPluginError } from "hardhat/internal/core/errors";
import { registerCompilerArgs, registerProjectPathArgs } from "../common";
import { ForgeBuildArgs, spawnBuild } from "./build";

registerProjectPathArgs(registerCompilerArgs(task("compile")))
  .setDescription("Compiles the entire project with forge")
  .addFlag(
    "offline",
    "Do not access the network. Missing solc versions will not be installed."
  )
  .addFlag(
    "viaIr",
    "Use the Yul intermediate representation compilation pipeline."
  )
  .setAction(async (args, hre, runSuper) => {
    const input = { ...args, ...(hre.config.foundry || {}) };
    const buildArgs = await getCheckedArgs(input);
    await spawnBuild(buildArgs);

    if (hre.config.foundry?.writeArtifacts!) {
      (hre as any).artifacts.writeArtifactsSync();
    }

    if (hre.config.foundry?.runSuper!) {
      await runSuper(args);
    }
  });

async function getCheckedArgs(args: any): Promise<ForgeBuildArgs> {
  // Get and initialize option validator
  const { default: buildArgsSchema } = await import("./build-ti");
  const { default: compilerArgsSchema } = await import("../common/compiler-ti");
  const { default: projectPathsSchema } = await import(
    "../common/projectpaths-ti"
  );
  const { createCheckers } = await import("ts-interface-checker");
  const { ForgeBuildArgsTi } = createCheckers(
    buildArgsSchema,
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
  return uncheckedBuildArgs as ForgeBuildArgs;
}
