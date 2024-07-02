import { UpgradesError } from '@openzeppelin/upgrades-core';
import { HardhatRuntimeEnvironment } from 'hardhat/types';

import { request } from 'undici';

import debug from './debug';
import { Etherscan } from '@nomicfoundation/hardhat-verify/etherscan';

/**
 * Call the configured Etherscan API with the given parameters.
 *
 * @param etherscan Etherscan instance
 * @param params The API parameters to call with
 * @returns The Etherscan API response
 */
export async function callEtherscanApi(etherscan: Etherscan, params: any): Promise<EtherscanResponseBody> {
  const parameters = new URLSearchParams({ ...params, apikey: etherscan.apiKey });

  const response = await request(etherscan.apiUrl, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: parameters.toString(),
  });

  if (!(response.statusCode >= 200 && response.statusCode <= 299)) {
    const responseBodyText = await response.body.text();
    throw new UpgradesError(
      `Etherscan API call failed with status ${response.statusCode}, response: ${responseBodyText}`,
    );
  }

  const responseBodyJson = await response.body.json();
  debug('Etherscan response', JSON.stringify(responseBodyJson));

  return responseBodyJson;
}

/**
 * Gets an Etherscan instance based on Hardhat config.
 * Throws an error if Etherscan API key is not present in config.
 */
export async function getEtherscanInstance(hre: HardhatRuntimeEnvironment): Promise<Etherscan> {
  const etherscanConfig: EtherscanConfig | undefined = (hre.config as any).etherscan; // This should never be undefined, but check just in case
  const chainConfig = await Etherscan.getCurrentChainConfig(
    hre.network.name,
    hre.network.provider,
    etherscanConfig?.customChains ?? [],
  );

  return Etherscan.fromChainConfig(etherscanConfig?.apiKey, chainConfig);
}

/**
 * Etherscan configuration for hardhat-verify.
 */
interface EtherscanConfig {
  apiKey: string | Record<string, string>;
  customChains: any[];
}

/**
 * The response body from an Etherscan API call.
 */
interface EtherscanResponseBody {
  status: string;
  message: string;
  result: any;
}

export const RESPONSE_OK = '1';

export async function verifyAndGetStatus(
  params: {
    contractAddress: string;
    sourceCode: string;
    contractName: string;
    compilerVersion: string;
    constructorArguments: string;
  },
  etherscan: Etherscan,
) {
  const response = await etherscan.verify(
    params.contractAddress,
    params.sourceCode,
    params.contractName,
    params.compilerVersion,
    params.constructorArguments,
  );
  return etherscan.getVerificationStatus(response.message);
}
