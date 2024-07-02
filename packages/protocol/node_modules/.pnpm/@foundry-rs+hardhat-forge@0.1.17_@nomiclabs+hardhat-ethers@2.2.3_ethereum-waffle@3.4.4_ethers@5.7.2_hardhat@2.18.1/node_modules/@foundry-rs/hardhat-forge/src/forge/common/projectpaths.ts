// bindings for common project paths settings

import { types } from "hardhat/config";
import { ConfigurableTaskDefinition } from "hardhat/types";

/**
 * Registers all `CompilerArgs` on the hardhat `ConfigurableTaskDefinition`
 * @param task
 */
export function registerProjectPathArgs(
  task: ConfigurableTaskDefinition
): ConfigurableTaskDefinition {
  return task
    .addOptionalParam(
      "root",
      "The project's root path.",
      undefined,
      types.string
    )
    .addOptionalParam(
      "contracts",
      "The contracts source directory.",
      undefined,
      types.string
    )
    .addOptionalParam(
      "remappings",
      "The project's remappings",
      undefined,
      types.string
    )
    .addOptionalParam(
      "cachePath",
      "The path to the compiler cache.",
      undefined,
      types.string
    )
    .addOptionalParam(
      "libPaths",
      "The path to the library folder.",
      undefined,
      types.string
    )
    .addFlag("hardhat", "Use the Hardhat-style project layout.")
    .addOptionalParam(
      "configPath",
      "Path to the config file.",
      undefined,
      types.string
    );
}

/**
 * Mirrors the `ProjectPaths` type
 */
export declare interface ProjectPathArgs {
  root?: string;
  contracts?: string;
  remappings?: string[];
  remappingsEnv?: string;
  cachePath?: string;
  libPaths?: string;
  hardhat?: boolean;
  configPath?: string;
  outPath?: string;
}

export function projectPathsArgs(args: ProjectPathArgs): string[] {
  const allArgs: string[] = [];

  const root = args.root ?? "";
  if (root) {
    allArgs.push("--root", root);
  }
  const contracts = args.contracts ?? "";
  if (contracts) {
    allArgs.push("--contracts", contracts);
  }
  if (args.remappings && args.remappings.length) {
    allArgs.push("--remappings", ...args.remappings);
  }
  const remappingsEnv = args.remappingsEnv ?? "";
  if (remappingsEnv) {
    allArgs.push("--remappings-env", remappingsEnv);
  }
  const cachePath = args.cachePath ?? "";
  if (cachePath) {
    allArgs.push("--cache-path", cachePath);
  }
  const libPaths = args.libPaths ?? "";
  if (libPaths) {
    allArgs.push("--lib-paths", libPaths);
  }
  if (args.hardhat === true) {
    allArgs.push("--hardhat");
  }
  const configPath = args.configPath ?? "";
  if (configPath) {
    allArgs.push("--config-path", configPath);
  }
  const outPath = args.outPath ?? "";
  if (outPath) {
    allArgs.push("--out", outPath);
  }

  return allArgs;
}
