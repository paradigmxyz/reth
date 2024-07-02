import { HardhatRuntimeEnvironment } from 'hardhat/types';

import { getNetwork, getDeployClient } from './utils';

export interface ApprovalProcess {
  approvalProcessId: string;
  address?: string;
}

export type GetDefaultApprovalProcessFunction = () => Promise<ApprovalProcess>;

export function makeGetDefaultApprovalProcess(hre: HardhatRuntimeEnvironment): GetDefaultApprovalProcessFunction {
  return async function getDefaultApprovalProcess() {
    const client = getDeployClient(hre);
    const network = await getNetwork(hre);

    const response = await client.getUpgradeApprovalProcess(network);

    if (response.network !== network) {
      // This should not happen
      throw new Error(
        `Returned an approval process for network ${response.network} which does not match current network ${network}`,
      );
    }

    return {
      approvalProcessId: response.approvalProcessId,
      address: response.via,
    };
  };
}
