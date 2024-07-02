import { HardhatRuntimeEnvironment } from 'hardhat/types';
import type { ContractFactory, ethers } from 'ethers';

import { DeployImplementationOptions } from './utils';
import { deployUpgradeableImpl } from './utils/deploy-impl';
import { enableDefender } from './defender/utils';

export type DeployImplementationFunction = (
  ImplFactory: ContractFactory,
  opts?: DeployImplementationOptions,
) => Promise<DeployImplementationResponse>;

export type DeployImplementationResponse = string | ethers.TransactionResponse;

export function makeDeployImplementation(
  hre: HardhatRuntimeEnvironment,
  defenderModule: boolean,
): DeployImplementationFunction {
  return async function deployImplementation(ImplFactory, opts: DeployImplementationOptions = {}) {
    opts = enableDefender(hre, defenderModule, opts);

    const deployedImpl = await deployUpgradeableImpl(hre, ImplFactory, opts);

    if (opts.getTxResponse && deployedImpl.txResponse) {
      return deployedImpl.txResponse;
    } else {
      return deployedImpl.impl;
    }
  };
}
