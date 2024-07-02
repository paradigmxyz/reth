import { HardhatRuntimeEnvironment } from 'hardhat/types';
import type { ContractFactory } from 'ethers';

import { validateImpl } from './utils/validate-impl';
import { getDeployData } from './utils/deploy-impl';
import { ValidateImplementationOptions } from './utils';

export type ValidateImplementationFunction = (
  ImplFactory: ContractFactory,
  opts?: ValidateImplementationOptions,
) => Promise<void>;

export function makeValidateImplementation(hre: HardhatRuntimeEnvironment): ValidateImplementationFunction {
  return async function validateImplementation(ImplFactory, opts: ValidateImplementationOptions = {}) {
    const deployData = await getDeployData(hre, ImplFactory, opts);
    await validateImpl(deployData, opts);
  };
}
