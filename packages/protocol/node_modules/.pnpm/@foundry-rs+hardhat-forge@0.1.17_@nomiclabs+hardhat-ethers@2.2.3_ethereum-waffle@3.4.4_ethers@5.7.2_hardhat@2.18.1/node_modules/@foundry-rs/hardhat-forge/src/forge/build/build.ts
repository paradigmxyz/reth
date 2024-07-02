// bindings for forge build
import { spawn as spawn } from "child_process";
import * as foundryup from "@foundry-rs/easy-foundryup";
import {
  compilerArgs,
  CompilerArgs,
  ProjectPathArgs,
  projectPathsArgs,
} from "../common";

/**
 * Mirrors the `forge build` arguments
 */
export declare interface ForgeBuildArgs extends CompilerArgs, ProjectPathArgs {
  force?: boolean;
  names?: boolean;
  sizes?: boolean;
  libraries?: string[];
  ignoredErrorCodes?: number[];
  noAutodetect?: boolean;
  useSolc?: string;
  offline?: boolean;
  viaIr?: boolean;
  buildInfo?: boolean;
  buildInfoPath?: string;
}

/** *
 * Invokes `forge build`
 * @param opts The arguments to pass to `forge build`
 */
export async function spawnBuild(opts: ForgeBuildArgs): Promise<boolean> {
  const args = ["build", ...buildArgs(opts)];
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
 * Converts the `args` object into a list of arguments for the `forge build` command
 * @param args
 */
export function buildArgs(args: ForgeBuildArgs): string[] {
  const allArgs: string[] = [];
  if (args.force === true) {
    allArgs.push("--force");
  }
  if (args.names === true) {
    allArgs.push("--names");
  }
  if (args.sizes === true) {
    allArgs.push("--sizes");
  }
  if (args.libraries && args.libraries.length) {
    allArgs.push("--libraries", ...args.libraries);
  }
  if (args.ignoredErrorCodes && args.ignoredErrorCodes.length) {
    const codes = args.ignoredErrorCodes.map((code) => code.toString());
    allArgs.push("--ignored-error-codes", ...codes);
  }
  if (args.noAutodetect === true) {
    allArgs.push("--no-auto-detect");
  }
  const useSolc = args.useSolc ?? "";
  if (useSolc) {
    allArgs.push("--use", useSolc);
  }
  if (args.offline === true) {
    allArgs.push("--offline");
  }
  if (args.viaIr === true) {
    allArgs.push("--via-ir");
  }
  if (args.buildInfo === true) {
    allArgs.push("--build-info");
  }
  if (typeof args.buildInfoPath === "string") {
    allArgs.push("--build-info-path", args.buildInfoPath);
  }

  allArgs.push(...compilerArgs(args));
  allArgs.push(...projectPathsArgs(args));

  return allArgs;
}
