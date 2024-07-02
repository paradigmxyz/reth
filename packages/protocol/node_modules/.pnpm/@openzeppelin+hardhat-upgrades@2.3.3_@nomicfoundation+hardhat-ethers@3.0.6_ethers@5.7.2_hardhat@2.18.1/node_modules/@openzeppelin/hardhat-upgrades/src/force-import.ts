import type { EthereumProvider, HardhatRuntimeEnvironment } from 'hardhat/types';
import type { ContractFactory, Contract } from 'ethers';

import {
  Manifest,
  getImplementationAddressFromProxy,
  getAdminAddress,
  addProxyToManifest,
  isBeacon,
  getImplementationAddressFromBeacon,
  inferProxyKind,
  isBeaconProxy,
  ProxyDeployment,
  hasCode,
  NoContractImportError,
  isEmptySlot,
  UpgradesError,
} from '@openzeppelin/upgrades-core';

import {
  simulateDeployImpl,
  ContractAddressOrInstance,
  getContractAddress,
  getUpgradeableBeaconFactory,
  ForceImportOptions,
} from './utils';
import { simulateDeployAdmin } from './utils/simulate-deploy';
import { getDeployData } from './utils/deploy-impl';
import { attach, getSigner } from './utils/ethers';

export interface ForceImportFunction {
  (proxyAddress: string, ImplFactory: ContractFactory, opts?: ForceImportOptions): Promise<Contract>;
}

export function makeForceImport(hre: HardhatRuntimeEnvironment): ForceImportFunction {
  return async function forceImport(
    addressOrInstance: ContractAddressOrInstance,
    ImplFactory: ContractFactory,
    opts: ForceImportOptions = {},
  ) {
    const { provider } = hre.network;
    const manifest = await Manifest.forNetwork(provider);

    const address = await getContractAddress(addressOrInstance);

    const implAddress = await getImplementationAddressFromProxy(provider, address);
    if (implAddress !== undefined) {
      await importProxyToManifest(provider, hre, address, implAddress, ImplFactory, opts, manifest);

      return attach(ImplFactory, address);
    } else if (await isBeacon(provider, address)) {
      const beaconImplAddress = await getImplementationAddressFromBeacon(provider, address);
      await addImplToManifest(hre, beaconImplAddress, ImplFactory, opts);

      const UpgradeableBeaconFactory = await getUpgradeableBeaconFactory(hre, getSigner(ImplFactory.runner));
      return attach(UpgradeableBeaconFactory, address);
    } else {
      if (!(await hasCode(provider, address))) {
        throw new NoContractImportError(address);
      }
      await addImplToManifest(hre, address, ImplFactory, opts);
      return attach(ImplFactory, address);
    }
  };
}

async function importProxyToManifest(
  provider: EthereumProvider,
  hre: HardhatRuntimeEnvironment,
  proxyAddress: string,
  implAddress: string,
  ImplFactory: ContractFactory,
  opts: ForceImportOptions,
  manifest: Manifest,
) {
  await addImplToManifest(hre, implAddress, ImplFactory, opts);

  let importKind: ProxyDeployment['kind'];
  if (opts.kind === undefined) {
    if (await isBeaconProxy(provider, proxyAddress)) {
      importKind = 'beacon';
    } else {
      const deployData = await getDeployData(hre, ImplFactory, opts);
      importKind = inferProxyKind(deployData.validations, deployData.version);
    }
  } else {
    importKind = opts.kind;
  }

  if (importKind === 'transparent') {
    await addAdminToManifest(provider, hre, proxyAddress, ImplFactory, opts);
  }
  await addProxyToManifest(importKind, proxyAddress, manifest);
}

async function addImplToManifest(
  hre: HardhatRuntimeEnvironment,
  implAddress: string,
  ImplFactory: ContractFactory,
  opts: ForceImportOptions,
) {
  await simulateDeployImpl(hre, ImplFactory, opts, implAddress);
}

async function addAdminToManifest(
  provider: EthereumProvider,
  hre: HardhatRuntimeEnvironment,
  proxyAddress: string,
  ImplFactory: ContractFactory,
  opts: ForceImportOptions,
) {
  const adminAddress = await getAdminAddress(provider, proxyAddress);
  if (isEmptySlot(adminAddress)) {
    // Assert that the admin slot of a transparent proxy is not zero, otherwise the simulation below would fail due to no code at the address.
    // Note: Transparent proxies should not have the zero address as the admin, according to TransparentUpgradeableProxy's _setAdmin function.
    throw new UpgradesError(
      `Proxy at ${proxyAddress} doesn't look like a transparent proxy`,
      () =>
        `The proxy doesn't look like a transparent proxy because its admin address slot is empty. ` +
        `Set the \`kind\` option to the kind of proxy that was deployed at ${proxyAddress} (either 'uups' or 'beacon')`,
    );
  }
  await simulateDeployAdmin(hre, ImplFactory, opts, adminAddress);
}
