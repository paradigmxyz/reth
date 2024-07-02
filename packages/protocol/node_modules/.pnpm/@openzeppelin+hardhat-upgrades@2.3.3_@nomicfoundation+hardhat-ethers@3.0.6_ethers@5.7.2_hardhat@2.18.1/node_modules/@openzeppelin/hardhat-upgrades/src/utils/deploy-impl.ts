import {
  fetchOrDeployGetDeployment,
  getStorageLayout,
  getUnlinkedBytecode,
  getVersion,
  StorageLayout,
  UpgradesError,
  ValidationDataCurrent,
  ValidationOptions,
  Version,
} from '@openzeppelin/upgrades-core';
import type { ContractFactory, ethers } from 'ethers';
import type { EthereumProvider, HardhatRuntimeEnvironment } from 'hardhat/types';
import { deploy } from './deploy';
import { GetTxResponse, DefenderDeployOptions, StandaloneOptions, UpgradeOptions, withDefaults } from './options';
import { getRemoteDeployment } from '../defender/utils';
import { validateBeaconImpl, validateProxyImpl, validateImpl } from './validate-impl';
import { readValidations } from './validations';

export interface DeployedImpl {
  impl: string;
  txResponse?: ethers.TransactionResponse;
}

export interface DeployedProxyImpl extends DeployedImpl {
  kind: NonNullable<ValidationOptions['kind']>;
}

export interface DeployData {
  provider: EthereumProvider;
  validations: ValidationDataCurrent;
  unlinkedBytecode: string;
  encodedArgs: string;
  version: Version;
  layout: StorageLayout;
  fullOpts: Required<UpgradeOptions>;
}

export async function getDeployData(
  hre: HardhatRuntimeEnvironment,
  ImplFactory: ContractFactory,
  opts: UpgradeOptions,
): Promise<DeployData> {
  const { provider } = hre.network;
  const validations = await readValidations(hre);
  const unlinkedBytecode = getUnlinkedBytecode(validations, ImplFactory.bytecode);
  const encodedArgs = ImplFactory.interface.encodeDeploy(opts.constructorArgs);
  const version = getVersion(unlinkedBytecode, ImplFactory.bytecode, encodedArgs);
  const layout = getStorageLayout(validations, version);
  const fullOpts = withDefaults(opts);
  return { provider, validations, unlinkedBytecode, encodedArgs, version, layout, fullOpts };
}

export async function deployUpgradeableImpl(
  hre: HardhatRuntimeEnvironment,
  ImplFactory: ContractFactory,
  opts: StandaloneOptions,
  currentImplAddress?: string,
): Promise<DeployedImpl> {
  const deployData = await getDeployData(hre, ImplFactory, opts);
  await validateImpl(deployData, opts, currentImplAddress);
  return await deployImpl(hre, deployData, ImplFactory, opts);
}

export async function deployProxyImpl(
  hre: HardhatRuntimeEnvironment,
  ImplFactory: ContractFactory,
  opts: UpgradeOptions,
  proxyAddress?: string,
): Promise<DeployedProxyImpl> {
  const deployData = await getDeployData(hre, ImplFactory, opts);
  await validateProxyImpl(deployData, opts, proxyAddress);
  if (opts.kind === undefined) {
    throw new Error('Broken invariant: Proxy kind is undefined');
  }
  return {
    ...(await deployImpl(hre, deployData, ImplFactory, opts)),
    kind: opts.kind,
  };
}

export async function deployBeaconImpl(
  hre: HardhatRuntimeEnvironment,
  ImplFactory: ContractFactory,
  opts: UpgradeOptions,
  beaconAddress?: string,
): Promise<DeployedImpl> {
  const deployData = await getDeployData(hre, ImplFactory, opts);
  await validateBeaconImpl(deployData, opts, beaconAddress);
  return await deployImpl(hre, deployData, ImplFactory, opts);
}

async function deployImpl(
  hre: HardhatRuntimeEnvironment,
  deployData: DeployData,
  ImplFactory: ContractFactory,
  opts: UpgradeOptions & GetTxResponse & DefenderDeployOptions,
): Promise<DeployedImpl> {
  const layout = deployData.layout;

  if (opts.useDeployedImplementation && opts.redeployImplementation !== undefined) {
    throw new UpgradesError(
      'The useDeployedImplementation and redeployImplementation options cannot both be set at the same time',
    );
  }

  const merge = deployData.fullOpts.redeployImplementation === 'always';

  const deployment = await fetchOrDeployGetDeployment(
    deployData.version,
    deployData.provider,
    async () => {
      const abi = ImplFactory.interface.format(true);
      const attemptDeploy = () => {
        if (deployData.fullOpts.useDeployedImplementation || deployData.fullOpts.redeployImplementation === 'never') {
          throw new UpgradesError('The implementation contract was not previously deployed.', () => {
            if (deployData.fullOpts.useDeployedImplementation) {
              return 'The useDeployedImplementation option was set to true but the implementation contract was not previously deployed on this network.';
            } else {
              return "The redeployImplementation option was set to 'never' but the implementation contract was not previously deployed on this network.";
            }
          });
        } else {
          return deploy(hre, opts, ImplFactory, ...deployData.fullOpts.constructorArgs);
        }
      };
      const deployment = Object.assign({ abi }, await attemptDeploy());
      return { ...deployment, layout };
    },
    opts,
    merge,
    remoteDeploymentId => getRemoteDeployment(hre, remoteDeploymentId),
  );

  let txResponse;
  if (opts.getTxResponse) {
    if ('deployTransaction' in deployment) {
      txResponse = deployment.deployTransaction ?? undefined;
    } else if (deployment.txHash !== undefined) {
      txResponse = (await hre.ethers.provider.getTransaction(deployment.txHash)) ?? undefined;
    }
  }

  return { impl: deployment.address, txResponse };
}
