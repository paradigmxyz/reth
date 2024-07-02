import { HardhatRuntimeEnvironment } from 'hardhat/types';
import type { ContractFactory } from 'ethers';

import {
  ContractAddressOrInstance,
  getContractAddress,
  deployProxyImpl,
  deployBeaconImpl,
  PrepareUpgradeOptions,
} from './utils';
import {
  getBeaconAddress,
  isBeaconProxy,
  isTransparentOrUUPSProxy,
  isBeacon,
  PrepareUpgradeRequiresKindError,
} from '@openzeppelin/upgrades-core';
import { DeployImplementationResponse } from './deploy-implementation';
import { enableDefender } from './defender/utils';
import { deployUpgradeableImpl, DeployedImpl } from './utils/deploy-impl';

export type PrepareUpgradeFunction = (
  referenceAddressOrContract: ContractAddressOrInstance,
  ImplFactory: ContractFactory,
  opts?: PrepareUpgradeOptions,
) => Promise<DeployImplementationResponse>;

export function makePrepareUpgrade(hre: HardhatRuntimeEnvironment, defenderModule: boolean): PrepareUpgradeFunction {
  return async function prepareUpgrade(referenceAddressOrContract, ImplFactory, opts: PrepareUpgradeOptions = {}) {
    opts = enableDefender(hre, defenderModule, opts);

    const deployedImpl = await deployImplForUpgrade(hre, referenceAddressOrContract, ImplFactory, opts);

    if (opts.getTxResponse && deployedImpl.txResponse) {
      return deployedImpl.txResponse;
    } else {
      return deployedImpl.impl;
    }
  };
}

export async function deployImplForUpgrade(
  hre: HardhatRuntimeEnvironment,
  referenceAddressOrContract: ContractAddressOrInstance,
  ImplFactory: ContractFactory,
  opts: PrepareUpgradeOptions = {},
): Promise<DeployedImpl> {
  const referenceAddress = await getContractAddress(referenceAddressOrContract);
  const { provider } = hre.network;
  let deployedImpl;
  if (await isTransparentOrUUPSProxy(provider, referenceAddress)) {
    deployedImpl = await deployProxyImpl(hre, ImplFactory, opts, referenceAddress);
  } else if (await isBeaconProxy(provider, referenceAddress)) {
    const beaconAddress = await getBeaconAddress(provider, referenceAddress);
    deployedImpl = await deployBeaconImpl(hre, ImplFactory, opts, beaconAddress);
  } else if (await isBeacon(provider, referenceAddress)) {
    deployedImpl = await deployBeaconImpl(hre, ImplFactory, opts, referenceAddress);
  } else {
    if (opts.kind === undefined) {
      throw new PrepareUpgradeRequiresKindError();
    }
    deployedImpl = await deployUpgradeableImpl(hre, ImplFactory, opts, referenceAddress);
  }
  return deployedImpl;
}
