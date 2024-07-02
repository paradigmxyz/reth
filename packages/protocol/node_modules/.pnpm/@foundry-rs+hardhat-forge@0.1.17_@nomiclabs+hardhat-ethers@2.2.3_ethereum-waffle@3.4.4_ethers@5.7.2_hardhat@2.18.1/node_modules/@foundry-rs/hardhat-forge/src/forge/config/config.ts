/* eslint-disable @typescript-eslint/naming-convention */
export declare interface RpcStorageCaching {
  chains?: string;
  endpoints?: string;
}

/**
 * Represents the foundry config object
 */
export declare interface FoundryConfig {
  src: string;
  test: string;
  out: string;
  libs: string[];
  remappings: string[];
  libraries?: string[];
  cache?: boolean;
  cache_path?: string;
  force?: boolean;
  evm_version?: string;
  gas_reports?: string[];
  solc?: string;
  auto_detect_solc?: boolean;
  offline?: boolean;
  optimizer?: boolean;
  optimizer_runs?: number;
  optimizer_details?: any;
  verbosity?: number;
  eth_rpc_url?: string;
  etherscan_api_key?: string;
  ignored_error_codes?: number[];
  match_test?: string;
  no_match_test?: string;
  match_contract?: string;
  no_match_contract?: string;
  match_path?: string;
  no_match_path?: string;
  fuzz_runs?: number;
  ffi?: boolean;
  sender?: string;
  tx_origin?: string;
  initial_balance?: string;
  block_number?: number;
  fork_block_number?: any;
  chain_id?: any;
  gas_limit?: number;
  gas_price?: number;
  block_base_fee_per_gas?: number;
  block_coinbase?: string;
  block_timestamp?: number;
  block_difficulty?: number;
  block_gas_limit?: any;
  memory_limit?: number;
  extra_output?: string[];
  extra_output_files?: string[];
  fuzz_max_local_rejects?: number;
  fuzz_max_global_rejects?: number;
  names?: boolean;
  sizes?: boolean;
  via_ir?: boolean;
  rpc_storage_caching?: RpcStorageCaching;
  no_storage_caching?: boolean;
  bytecode_hash?: string;
  revert_strings?: any;
  sparse_mode?: boolean;
  build_info?: boolean;
  build_info_path?: string;
}
