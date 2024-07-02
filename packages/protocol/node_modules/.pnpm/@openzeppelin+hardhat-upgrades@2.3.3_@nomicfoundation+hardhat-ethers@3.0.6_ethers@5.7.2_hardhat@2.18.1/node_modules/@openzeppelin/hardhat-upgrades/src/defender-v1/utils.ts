import { AdminClient } from '@openzeppelin/defender-admin-client';
import { fromChainId, Network } from '@openzeppelin/defender-base-client';
import { HardhatRuntimeEnvironment } from 'hardhat/types';

export function getAdminClient(hre: HardhatRuntimeEnvironment): AdminClient {
  const cfg = hre.config.defender;
  if (!cfg || !cfg.apiKey || !cfg.apiSecret) {
    const sampleConfig = JSON.stringify({ apiKey: 'YOUR_API_KEY', apiSecret: 'YOUR_API_SECRET' }, null, 2);
    throw new Error(
      `Missing Defender API key and secret in hardhat config. Add the following to your hardhat.config.js configuration:\ndefender: ${sampleConfig}\n`,
    );
  }
  return new AdminClient(cfg);
}

export async function getNetwork(hre: HardhatRuntimeEnvironment): Promise<Network> {
  const id = await hre.network.provider.send('eth_chainId', []);
  const chainId = parseInt(id.replace(/^0x/, ''), 16);
  const network = fromChainId(chainId);
  if (network === undefined) {
    throw new Error(`Network ${chainId} is not supported in Defender`);
  }
  return network;
}
