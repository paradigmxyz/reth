// bindings for common compiler settings
import { types } from "hardhat/config";
import { ConfigurableTaskDefinition } from "hardhat/types";

/**
 * Registers all `CompilerArgs` on the hardhat `ConfigurableTaskDefinition`
 * @param task
 */
export function registerCompilerArgs(
  task: ConfigurableTaskDefinition
): ConfigurableTaskDefinition {
  return task
    .addOptionalParam(
      "evmVersion",
      "The target EVM version.",
      undefined,
      types.string
    )
    .addFlag("optimize", "Activate the Solidity optimizer.")
    .addOptionalParam(
      "optimizerRuns",
      "The number of optimizer runs.",
      undefined,
      types.int
    );
}

/**
 * Mirrors the `CompilerArgs` type
 */
export declare interface CompilerArgs {
  evmVersion?: string;
  optimize?: boolean;
  optimizerRuns?: number;
  extraOutput?: string[];
  extraOutputFiles?: string[];
}

/**
 * Transforms the `CompilerArgs` in to a list of comand arguments
 * @param args
 */
export function compilerArgs(args: CompilerArgs): string[] {
  const allArgs: string[] = [];

  const evmVersion = args.evmVersion ?? "";
  if (evmVersion) {
    allArgs.push("--evm-version", evmVersion);
  }

  if (args.optimize === true) {
    allArgs.push("--optimize");
    const optimizerRuns = args.optimizerRuns ?? 0;
    if (optimizerRuns) {
      allArgs.push("--optimizer-runs", optimizerRuns.toString());
    }
  }

  if (args.extraOutput && args.extraOutput.length) {
    allArgs.push("--extra-output", ...args.extraOutput);
  }

  if (args.extraOutputFiles && args.extraOutputFiles.length) {
    allArgs.push("--extra-output-files", ...args.extraOutputFiles);
  }

  return allArgs;
}
