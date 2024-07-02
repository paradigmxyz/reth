import { fetchOrDeploy, fetchOrDeployAdmin, logWarning } from '@openzeppelin/upgrades-core';
import type { ContractFactory } from 'ethers';
import type { HardhatRuntimeEnvironment } from 'hardhat/types';
import { getDeployData } from './deploy-impl';
import { UpgradeOptions } from './options';

// To import an already deployed contract we want to reuse fetchOrDeploy for its ability to validate
// a deployment and record it in the network file. We are able to do this by "simulating" a deployment:
// for the "deploy" part we pass a function that simply returns the contract to be imported, rather than
// actually deploying something.

export async function simulateDeployAdmin(
  hre: HardhatRuntimeEnvironment,
  ProxyAdminFactory: ContractFactory,
  opts: UpgradeOptions,
  adminAddress: string,
) {
  const { deployData, simulateDeploy } = await getSimulatedData(hre, ProxyAdminFactory, opts, adminAddress);
  const manifestAdminAddress = await fetchOrDeployAdmin(deployData.provider, simulateDeploy, opts);
  if (adminAddress !== manifestAdminAddress) {
    logWarning(
      `Imported proxy with admin at '${adminAddress}' which differs from previously deployed admin '${manifestAdminAddress}'`,
      [
        `The imported proxy admin is different from the proxy admin that was previously deployed on this network.`,
        `New proxy deployments will continue to use the admin '${manifestAdminAddress}'.`,
      ],
    );
  }
}

export async function simulateDeployImpl(
  hre: HardhatRuntimeEnvironment,
  ImplFactory: ContractFactory,
  opts: UpgradeOptions,
  implAddress: string,
) {
  const { deployData, simulateDeploy } = await getSimulatedData(hre, ImplFactory, opts, implAddress);
  await fetchOrDeploy(deployData.version, deployData.provider, simulateDeploy, opts, true);
}

/**
 * Gets data for a simulated deployment of the given contract to the given address.
 */
async function getSimulatedData(
  hre: HardhatRuntimeEnvironment,
  ImplFactory: ContractFactory,
  opts: UpgradeOptions,
  implAddress: string,
) {
  const deployData = await getDeployData(hre, ImplFactory, opts);
  const simulateDeploy = async () => {
    return {
      abi: ImplFactory.interface.format(true),
      layout: deployData.layout,
      address: implAddress,
    };
  };
  return { deployData, simulateDeploy };
}
