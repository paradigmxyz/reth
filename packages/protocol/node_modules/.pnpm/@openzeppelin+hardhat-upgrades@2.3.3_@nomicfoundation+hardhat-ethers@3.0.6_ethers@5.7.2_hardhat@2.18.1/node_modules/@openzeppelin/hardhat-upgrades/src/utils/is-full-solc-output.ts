import type { SolcOutput } from '@openzeppelin/upgrades-core';

type RecursivePartial<T> = { [k in keyof T]?: RecursivePartial<T[k]> };

type MaybeSolcOutput = RecursivePartial<SolcOutput>;

export function isFullSolcOutput(output: MaybeSolcOutput | undefined): boolean {
  if (output?.contracts == undefined || output?.sources == undefined) {
    return false;
  }

  for (const file of Object.values(output.contracts)) {
    if (file == undefined) {
      return false;
    }
    for (const contract of Object.values(file)) {
      if (contract?.evm?.bytecode == undefined) {
        return false;
      }
    }
  }

  for (const file of Object.values(output.sources)) {
    if (file?.ast == undefined || file?.id == undefined) {
      return false;
    }
  }

  return true;
}
