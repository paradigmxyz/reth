import { Network } from '@openzeppelin/defender-base-client';
export type Address = string;
export interface Contract {
    network: Network;
    address: Address;
    name: string;
    abi?: string;
    natSpec?: string;
}
//# sourceMappingURL=contract.d.ts.map