import { HardhatRuntimeEnvironment } from 'hardhat/types';
import type { ethers, ContractFactory, Contract, Signer } from 'ethers';

import { getAdminAddress, getCode, getUpgradeInterfaceVersion, isEmptySlot } from '@openzeppelin/upgrades-core';

import {
  UpgradeProxyOptions,
  deployProxyImpl,
  getITransparentUpgradeableProxyFactory,
  getProxyAdminFactory,
  getContractAddress,
  ContractAddressOrInstance,
  getSigner,
} from './utils';
import { disableDefender } from './defender/utils';
import { attach } from './utils/ethers';

export type UpgradeFunction = (
  proxy: ContractAddressOrInstance,
  ImplFactory: ContractFactory,
  opts?: UpgradeProxyOptions,
) => Promise<Contract>;

export function makeUpgradeProxy(hre: HardhatRuntimeEnvironment, defenderModule: boolean): UpgradeFunction {
  return async function upgradeProxy(proxy, ImplFactory, opts: UpgradeProxyOptions = {}) {
    disableDefender(hre, defenderModule, opts, upgradeProxy.name);

    const proxyAddress = await getContractAddress(proxy);

    const { impl: nextImpl } = await deployProxyImpl(hre, ImplFactory, opts, proxyAddress);
    // upgrade kind is inferred above
    const upgradeTo = await getUpgrader(proxyAddress, opts, getSigner(ImplFactory.runner));
    const call = encodeCall(ImplFactory, opts.call);
    const upgradeTx = await upgradeTo(nextImpl, call);

    const inst = attach(ImplFactory, proxyAddress);
    // @ts-ignore Won't be readonly because inst was created through attach.
    inst.deployTransaction = upgradeTx;
    return inst;
  };

  type Upgrader = (nextImpl: string, call?: string) => Promise<ethers.TransactionResponse>;

  async function getUpgrader(proxyAddress: string, opts: UpgradeProxyOptions, signer?: Signer): Promise<Upgrader> {
    const { provider } = hre.network;

    const adminAddress = await getAdminAddress(provider, proxyAddress);
    const adminBytecode = await getCode(provider, adminAddress);

    const overrides = opts.txOverrides ? [opts.txOverrides] : [];

    if (isEmptySlot(adminAddress) || adminBytecode === '0x') {
      // No admin contract: use ITransparentUpgradeableProxyFactory to get proxiable interface
      const ITransparentUpgradeableProxyFactory = await getITransparentUpgradeableProxyFactory(hre, signer);
      const proxy = attach(ITransparentUpgradeableProxyFactory, proxyAddress);

      const upgradeInterfaceVersion = await getUpgradeInterfaceVersion(provider, proxyAddress);

      return (nextImpl, call) => {
        if (upgradeInterfaceVersion === undefined) {
          return call ? proxy.upgradeToAndCall(nextImpl, call, ...overrides) : proxy.upgradeTo(nextImpl, ...overrides);
        } else if (upgradeInterfaceVersion === '5.0.0') {
          return proxy.upgradeToAndCall(nextImpl, call ?? '0x', ...overrides);
        } else {
          throw new Error(
            `Unknown UPGRADE_INTERFACE_VERSION ${upgradeInterfaceVersion} for proxy at ${proxyAddress}. Expected 5.0.0`,
          );
        }
      };
    } else {
      // Admin contract: redirect upgrade call through it
      const AdminFactory = await getProxyAdminFactory(hre, signer);
      const admin = attach(AdminFactory, adminAddress);

      const upgradeInterfaceVersion = await getUpgradeInterfaceVersion(provider, adminAddress);

      return (nextImpl, call) => {
        if (upgradeInterfaceVersion === undefined) {
          return call
            ? admin.upgradeAndCall(proxyAddress, nextImpl, call, ...overrides)
            : admin.upgrade(proxyAddress, nextImpl, ...overrides);
        } else if (upgradeInterfaceVersion === '5.0.0') {
          return admin.upgradeAndCall(proxyAddress, nextImpl, call ?? '0x', ...overrides);
        } else {
          throw new Error(
            `Unknown UPGRADE_INTERFACE_VERSION ${upgradeInterfaceVersion} for proxy admin at ${adminAddress}. Expected 5.0.0`,
          );
        }
      };
    }
  }
}

function encodeCall(factory: ContractFactory, call: UpgradeProxyOptions['call']): string | undefined {
  if (!call) {
    return undefined;
  }

  if (typeof call === 'string') {
    call = { fn: call };
  }

  return factory.interface.encodeFunctionData(call.fn, call.args ?? []);
}
