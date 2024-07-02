import { HardhatRuntimeEnvironment } from 'hardhat/types';
import type { ContractFactory, Contract } from 'ethers';

import { deploy, DeployContractOptions, DeployTransaction } from './utils';
import { DeployData, getDeployData } from './utils/deploy-impl';
import { enableDefender } from './defender/utils';
import {
  Deployment,
  RemoteDeploymentId,
  getContractNameAndRunValidation,
  inferProxyKind,
  UpgradesError,
  inferInitializable,
} from '@openzeppelin/upgrades-core';
import { getContractInstance } from './utils/contract-instance';

export interface DeployContractFunction {
  (Contract: ContractFactory, args?: unknown[], opts?: DeployContractOptions): Promise<Contract>;
  (Contract: ContractFactory, opts?: DeployContractOptions): Promise<Contract>;
}

async function deployNonUpgradeableContract(
  hre: HardhatRuntimeEnvironment,
  Contract: ContractFactory,
  opts: DeployContractOptions,
) {
  const deployData = await getDeployData(hre, Contract, opts);

  if (!opts.unsafeAllowDeployContract) {
    assertNonUpgradeable(deployData);
  }

  const deployment: Required<Deployment> & DeployTransaction & RemoteDeploymentId = await deploy(
    hre,
    opts,
    Contract,
    ...deployData.fullOpts.constructorArgs,
  );

  return deployment;
}

function assertNonUpgradeable(deployData: DeployData) {
  const [fullContractName, runValidation] = getContractNameAndRunValidation(deployData.validations, deployData.version);
  const c = runValidation[fullContractName];
  if (inferInitializable(c) || inferProxyKind(deployData.validations, deployData.version) === 'uups') {
    throw new UpgradesError(
      `The contract ${fullContractName} looks like an upgradeable contract.`,
      () =>
        'Upgradable contracts cannot be deployed using the deployContract function. Use deployProxy, deployBeacon, or deployImplementation.\n' +
        'If this is not intended to be an upgradeable contract, set the unsafeAllowDeployContract option to true and run the deployContract function again.',
    );
  }
}

export function makeDeployContract(hre: HardhatRuntimeEnvironment, defenderModule: boolean): DeployContractFunction {
  return async function deployContract(
    Contract,
    args: unknown[] | DeployContractOptions = [],
    opts: DeployContractOptions = {},
  ) {
    if (!Array.isArray(args)) {
      opts = args;
      args = [];
    }

    opts = enableDefender(hre, defenderModule, opts);

    if (!opts.useDefenderDeploy) {
      throw new Error(`The ${deployContract.name} function cannot have the \`useDefenderDeploy\` option disabled.`);
    }

    if (opts.constructorArgs !== undefined) {
      throw new Error(
        `The ${deployContract.name} function does not support the constructorArgs option. Pass in constructor arguments using the format: deployContract(MyContract, [ 'my arg' ]);`,
      );
    }
    opts.constructorArgs = args;

    const deployment = await deployNonUpgradeableContract(hre, Contract, opts);

    return getContractInstance(hre, Contract, opts, deployment);
  };
}
