import { HardhatRuntimeEnvironment } from 'hardhat/types';
import { ContractFactory } from 'ethers';

import { ContractAddressOrInstance, getContractAddress } from './utils';
import {
  getBeaconAddress,
  isBeaconProxy,
  isTransparentOrUUPSProxy,
  isBeacon,
  assertUpgradeSafe,
  assertStorageUpgradeSafe,
  ValidationOptions,
  inferProxyKind,
  ValidateUpdateRequiresKindError,
} from '@openzeppelin/upgrades-core';
import { validateBeaconImpl, validateImpl, validateProxyImpl } from './utils/validate-impl';
import { getDeployData } from './utils/deploy-impl';

export interface ValidateUpgradeFunction {
  (origImplFactory: ContractFactory, newImplFactory: ContractFactory, opts?: ValidationOptions): Promise<void>;
  (
    proxyOrBeaconAddress: ContractAddressOrInstance,
    newImplFactory: ContractFactory,
    opts?: ValidationOptions,
  ): Promise<void>;
}

export function makeValidateUpgrade(hre: HardhatRuntimeEnvironment): ValidateUpgradeFunction {
  return async function validateUpgrade(
    referenceAddressOrImplFactory: ContractAddressOrInstance | ContractFactory,
    newImplFactory: ContractFactory,
    opts: ValidationOptions = {},
  ) {
    if (referenceAddressOrImplFactory instanceof ContractFactory) {
      const origDeployData = await getDeployData(hre, referenceAddressOrImplFactory, opts);
      if (opts.kind === undefined) {
        opts.kind = inferProxyKind(origDeployData.validations, origDeployData.version);
      }

      const newDeployData = await getDeployData(hre, newImplFactory, opts);
      assertUpgradeSafe(newDeployData.validations, newDeployData.version, newDeployData.fullOpts);

      if (opts.unsafeSkipStorageCheck !== true) {
        assertStorageUpgradeSafe(origDeployData.layout, newDeployData.layout, newDeployData.fullOpts);
      }
    } else {
      const referenceAddress = await getContractAddress(referenceAddressOrImplFactory);
      const { provider } = hre.network;
      const deployData = await getDeployData(hre, newImplFactory, opts);
      if (await isTransparentOrUUPSProxy(provider, referenceAddress)) {
        await validateProxyImpl(deployData, opts, referenceAddress);
      } else if (await isBeaconProxy(provider, referenceAddress)) {
        const beaconAddress = await getBeaconAddress(provider, referenceAddress);
        await validateBeaconImpl(deployData, opts, beaconAddress);
      } else if (await isBeacon(provider, referenceAddress)) {
        await validateBeaconImpl(deployData, opts, referenceAddress);
      } else {
        if (opts.kind === undefined) {
          throw new ValidateUpdateRequiresKindError();
        }
        await validateImpl(deployData, opts, referenceAddress);
      }
    }
  };
}
