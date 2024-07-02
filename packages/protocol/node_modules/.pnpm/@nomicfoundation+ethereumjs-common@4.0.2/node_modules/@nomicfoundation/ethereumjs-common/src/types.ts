import type { Chain, ConsensusAlgorithm, ConsensusType, Hardfork } from './enums'

export interface ChainName {
  [chainId: string]: string
}
export interface ChainsConfig {
  [key: string]: ChainConfig | ChainName
}

export type CliqueConfig = {
  period: number
  epoch: number
}

export type EthashConfig = {}

export type CasperConfig = {}
export interface ChainConfig {
  name: string
  chainId: number | bigint
  networkId: number | bigint
  defaultHardfork?: string
  comment?: string
  url?: string
  genesis: GenesisBlockConfig
  hardforks: HardforkConfig[]
  bootstrapNodes: BootstrapNodeConfig[]
  dnsNetworks?: string[]
  consensus: {
    type: ConsensusType | string
    algorithm: ConsensusAlgorithm | string
    clique?: CliqueConfig
    ethash?: EthashConfig
    casper?: CasperConfig
  }
}

export interface GenesisBlockConfig {
  timestamp?: string
  gasLimit: number
  difficulty: number
  nonce: string
  extraData: string
  baseFeePerGas?: string
}

export interface HardforkConfig {
  name: Hardfork | string
  block: number | null // null is used for hardforks that should not be applied -- since `undefined` isn't a valid value in JSON
  ttd?: bigint | string
  timestamp?: number | string
  forkHash?: string | null
}

export interface BootstrapNodeConfig {
  ip: string
  port: number | string
  network?: string
  chainId?: number
  id: string
  location: string
  comment: string
}

interface BaseOpts {
  /**
   * String identifier ('byzantium') for hardfork or {@link Hardfork} enum.
   *
   * Default: Hardfork.London
   */
  hardfork?: string | Hardfork
  /**
   * Selected EIPs which can be activated, please use an array for instantiation
   * (e.g. `eips: [ 2537, ]`)
   *
   * Currently supported:
   *
   * - [EIP-2537](https://eips.ethereum.org/EIPS/eip-2537) - BLS12-381 precompiles
   */
  eips?: number[]
}

/**
 * Options for instantiating a {@link Common} instance.
 */
export interface CommonOpts extends BaseOpts {
  /**
   * Chain name ('mainnet'), id (1), or {@link Chain} enum,
   * either from a chain directly supported or a custom chain
   * passed in via {@link CommonOpts.customChains}.
   */
  chain: string | number | Chain | bigint | object
  /**
   * Initialize (in addition to the supported chains) with the selected
   * custom chains. Custom genesis state should be passed to the Blockchain class if used.
   *
   * Usage (directly with the respective chain initialization via the {@link CommonOpts.chain} option):
   *
   * ```javascript
   * import myCustomChain1 from '[PATH_TO_MY_CHAINS]/myCustomChain1.json'
   * const common = new Common({ chain: 'myCustomChain1', customChains: [ myCustomChain1 ]})
   * ```
   */
  customChains?: ChainConfig[]
}

/**
 * Options to be used with the {@link Common.custom} static constructor.
 */
export interface CustomCommonOpts extends BaseOpts {
  /**
   * The name (`mainnet`), id (`1`), or {@link Chain} enum of
   * a standard chain used to base the custom chain params on.
   */
  baseChain?: string | number | Chain | bigint
}

export interface GethConfigOpts extends BaseOpts {
  chain?: string
  genesisHash?: Buffer
  mergeForkIdPostMerge?: boolean
}
