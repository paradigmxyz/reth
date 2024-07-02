import '../type-extensions';
import {
  getImplementationAddress,
  isBeaconProxy,
  isTransparentOrUUPSProxy,
  isTransparentProxy,
} from '@openzeppelin/upgrades-core';
import { ProposalResponse } from '@openzeppelin/defender-admin-client';
import { ContractFactory, getCreateAddress, ethers } from 'ethers';
import { HardhatRuntimeEnvironment } from 'hardhat/types';
import { getAdminClient, getNetwork } from './utils';
import type { VerificationResponse } from './verify-deployment';
import { UpgradeOptions } from '../utils/options';

export interface ExtendedProposalResponse extends ProposalResponse {
  txResponse?: ethers.TransactionResponse;
  verificationResponse?: VerificationResponse;
}

export type ProposeUpgradeFunction = (
  proxyAddress: string,
  contractNameOrImplFactory: string | ContractFactory,
  opts?: ProposalOptions,
) => Promise<ExtendedProposalResponse>;

export interface ProposalOptions extends UpgradeOptions {
  title?: string;
  description?: string;
  proxyAdmin?: string;
  multisig?: string;
  multisigType?: 'Gnosis Safe' | 'Gnosis Multisig' | 'EOA';
  bytecodeVerificationReferenceUrl?: string;
}

export function makeProposeUpgrade(hre: HardhatRuntimeEnvironment): ProposeUpgradeFunction {
  return async function proposeUpgrade(proxyAddress, contractNameOrImplFactory, opts = {}) {
    const client = getAdminClient(hre);
    const network = await getNetwork(hre);

    const { title, description, proxyAdmin, multisig, multisigType, ...moreOpts } = opts;

    if (await isBeaconProxy(hre.network.provider, proxyAddress)) {
      throw new Error(`Beacon proxy is not currently supported with defender.proposeUpgrade()`);
    } else if (
      !multisig &&
      (await isTransparentOrUUPSProxy(hre.network.provider, proxyAddress)) &&
      !(await isTransparentProxy(hre.network.provider, proxyAddress))
    ) {
      throw new Error(`Multisig address is a required property for UUPS proxies`);
    } else {
      // try getting the implementation address so that it will give an error if it's not a transparent/uups proxy
      await getImplementationAddress(hre.network.provider, proxyAddress);
    }

    const implFactory =
      typeof contractNameOrImplFactory === 'string'
        ? await hre.ethers.getContractFactory(contractNameOrImplFactory)
        : contractNameOrImplFactory;
    const contractName = typeof contractNameOrImplFactory === 'string' ? contractNameOrImplFactory : undefined;
    const contract = { address: proxyAddress, network, abi: implFactory.interface.formatJson() };

    const prepareUpgradeResult = await hre.upgrades.prepareUpgrade(proxyAddress, implFactory, {
      getTxResponse: true,
      ...moreOpts,
    });

    let txResponse, newImplementation;

    if (typeof prepareUpgradeResult === 'string') {
      newImplementation = prepareUpgradeResult;
    } else {
      txResponse = prepareUpgradeResult;
      newImplementation = getCreateAddress(txResponse);
    }

    const verificationResponse =
      contractName && opts.bytecodeVerificationReferenceUrl
        ? await hre.defender.verifyDeployment(newImplementation, contractName, opts.bytecodeVerificationReferenceUrl)
        : undefined;

    const proposalResponse = await client.proposeUpgrade(
      {
        newImplementation,
        title,
        description,
        proxyAdmin,
        via: multisig,
        viaType: multisigType,
      },
      contract,
    );

    return {
      ...proposalResponse,
      txResponse,
      verificationResponse,
    };
  };
}
