export type Network = SupportedNetwork | TenantNetwork;
export type SupportedNetwork = PublicNetwork | CustomNetwork;
export type PublicNetwork = 'mainnet' | 'sepolia' | 'holesky' | 'xdai' | 'sokol' | 'fuse' | 'bsc' | 'bsctest' | 'fantom' | 'fantomtest' | 'moonbase' | 'moonriver' | 'moonbeam' | 'matic' | 'mumbai' | 'amoy' | 'matic-zkevm' | 'matic-zkevm-testnet' | 'avalanche' | 'fuji' | 'arbitrum' | 'arbitrum-nova' | 'arbitrum-sepolia' | 'optimism' | 'optimism-sepolia' | 'celo' | 'alfajores' | 'harmony-s0' | 'harmony-test-s0' | 'aurora' | 'auroratest' | 'hedera' | 'hederatest' | 'zksync' | 'zksync-sepolia' | 'base' | 'base-sepolia' | 'linea-goerli' | 'linea' | 'mantle' | 'mantle-sepolia' | 'scroll' | 'scroll-sepolia' | 'meld' | 'meld-kanazawa';
export type CustomNetwork = 'x-dfk-avax-chain' | 'x-dfk-avax-chain-test' | 'x-security-alliance';
export type TenantNetwork = string;
export declare const Networks: Network[];
export declare function isValidNetwork(text: string): text is Network;
export declare function fromChainId(chainId: number): Network | undefined;
export declare function toChainId(network: Network): number | undefined;
//# sourceMappingURL=network.d.ts.map