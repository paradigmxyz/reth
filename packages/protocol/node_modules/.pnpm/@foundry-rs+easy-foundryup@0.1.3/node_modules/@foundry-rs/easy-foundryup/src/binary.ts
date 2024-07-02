import { exec, execSync, spawn } from "child_process";

const os = require("os");
const path = require("path");
const commandExists = require("command-exists");

const FOUNDRYUP_INSTALLER = 'curl -sSL "https://foundry.paradigm.xyz" | sh';

/**
 * @returns the path to the anvil path to use, if `anvil` is in path then this will be returned
 *
 */
export async function getAnvilCommand(): Promise<string> {
  try {
    return commandExists("anvil");
  } catch (e) {
    const cmd = foundryAnvilBinPath();
    await checkCommand(`${cmd} --version`);
    return cmd;
  }
}

/**
 * @returns the path to the cast path to use, if `cast` is in path then this will be returned
 *
 */
export async function getCastCommand(): Promise<string> {
  try {
    return commandExists("cast");
  } catch (e) {
    const cmd = foundryCastBinPath();
    await checkCommand(`${cmd} --version`);
    return cmd;
  }
}

/**
 * @returns the path to the forge path to use, if `forge` is in path then this will be returned
 *
 */
export async function getForgeCommand(): Promise<string> {
  try {
    return commandExists("forge");
  } catch (e) {
    const cmd = foundryForgeBinPath();
    await checkCommand(`${cmd} --version`);
    return cmd;
  }
}

/**
 * @returns the path to the forge path to use, if `forge` is in path then this will be returned
 *
 */
export function getForgeCommandSync(): string {
  if (commandExists.sync("forge")) {
    return "forge";
  } else {
    const cmd = foundryForgeBinPath();
    checkCommandSync(`${cmd} --version`);
    return cmd;
  }
}

/**
 * @returns the path to the foundry directory: `$HOME/.foundry`
 */
export function foundryDir(): string {
  return path.join(os.homedir(), ".foundry");
}

/**
 * @returns the path to the foundry directory that stores the tool binaries: `$HOME/.foundry/bin`
 */
export function foundryBinDir(): string {
  return path.join(foundryDir(), "bin");
}

/**
 * @returns the path to the anvil binary in the foundry dir: `$HOME/.foundry/bin/anvil`
 */
export function foundryAnvilBinPath(): string {
  return path.join(foundryDir(), "anvil");
}

/**
 * @returns the path to the cast binary in the foundry dir: `$HOME/.foundry/bin/cast`
 */
export function foundryCastBinPath(): string {
  return path.join(foundryDir(), "cast");
}

/**
 * @returns the path to the anvil forge in the foundry dir: `$HOME/.foundry/bin/forge`
 */
export function foundryForgeBinPath(): string {
  return path.join(foundryDir(), "forge");
}

/**
 * Installs foundryup via subprocess
 */
export async function selfInstall(): Promise<boolean> {
  return new Promise((resolve) => {
    const process = spawn("/bin/sh", ["-c", FOUNDRYUP_INSTALLER], {
      stdio: "inherit",
    });
    process.on("exit", (code) => {
      resolve(code === 0);
    });
  });
}

/**
 * Optional target location `foundryup` accepts
 */
export interface FoundryupTarget {
  branch?: string;
  commit?: string;
  repo?: string;
  path?: string;
}

/**
 * Executes `foundryup`
 *
 * @param install whether to install `foundryup` itself
 * @param _target additional `foundryup` params
 */
export async function run(
  install: boolean = true,
  _target: FoundryupTarget = {}
): Promise<boolean> {
  if (install) {
    if (!(await checkFoundryUp())) {
      if (!(await selfInstall())) {
        return false;
      }
    }
  }
  return checkCommand("foundryup");
}

/**
 * Checks if foundryup exists
 *
 * @return true if `foundryup` exists
 */
export async function checkFoundryUp(): Promise<boolean> {
  return checkCommand("foundryup --version");
}

/**
 * Checks if anvil exists
 *
 * @return true if `anvil` exists
 */
export async function checkAnvil(): Promise<boolean> {
  return checkCommand("anvil --version");
}

/**
 * Checks if cast exists
 *
 * @return true if `cast` exists
 */
export async function checkCast(): Promise<boolean> {
  return checkCommand("cast --version");
}

/**
 * Checks if cast exists
 *
 * @return true if `cast` exists
 */
export async function checkForge(): Promise<boolean> {
  return checkCommand("forge --version");
}

/**
 * Executes the given command
 *
 * @param cmd the command to run
 * @return returns true if the command succeeded, false otherwise
 */
async function checkCommand(cmd: string): Promise<boolean> {
  return new Promise((resolve) => {
    const process = exec(cmd);
    process.on("exit", (code) => {
      if (code !== 0) {
        console.error(
          "Command failed. Is Foundry not installed? Consider installing via `curl -L https://foundry.paradigm.xyz | bash` and then running `foundryup` on a new terminal. For more context, check the installation instructions in the book: https://book.getfoundry.sh/getting-started/installation.html."
        );
      }
      resolve(code === 0);
    });
  });
}

/**
 * Executes the given command
 *
 * @param cmd the command to run
 * @return returns true if the command succeeded, false otherwise
 */
function checkCommandSync(cmd: string): boolean {
  try {
    execSync(cmd);
    return true;
  } catch (error) {
    const status = (error as any).status === 0;
    if (!status) {
      console.error(
        "Command failed. Is Foundry not installed? Consider installing via `curl -L https://foundry.paradigm.xyz | bash` and then running `foundryup` on a new terminal. For more context, check the installation instructions in the book: https://book.getfoundry.sh/getting-started/installation.html."
      );
    }
    return status;
  }
}
