---
title: 'feat: configurable EVM execution limits'
labels:
    - C-enhancement
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.019123Z
info:
    author: rezzmah
    created_at: 2026-01-14T02:52:11Z
    updated_at: 2026-01-14T02:52:11Z
---

### Describe the feature

Many chains on existing stacks need flexibility around `MAX_CODE_SIZE`, `MAX_INITCODE_SIZE`, and `tx_gas_limit_cap`. These fields can be configured as part of revm `CfgEnv`, with supportive default behaviour, e.g. 
```rs
    fn max_code_size(&self) -> usize {
        self.limit_contract_code_size
            .unwrap_or(eip170::MAX_CODE_SIZE)
    }
```

 This need for customization is validated by [EIP-7907](https://eips.ethereum.org/EIPS/eip-7907).

Ofcourse, this can be done invasively by creating custom implementation of the relevant node components. However, such an approach is overly verbose for the simple requirement of increasing the max code size on op-stack.

The proposed approach is to add `EvmLimitParams` to the chain spec and expose this for fork aware access via
```rs
  pub struct EvmLimitParams {
      pub max_code_size: usize,
      pub max_initcode_size: usize,
      pub tx_gas_limit_cap: Option<u64>,
  }

fn evm_limit_params_at_timestamp(&self, timestamp: u64) -> EvmLimitParams
```
replacing the direct use of constants. Being forkaware seems necessary given that any updates to live networks would require all nodes to be aware of the correct limits for block validation. Execution `CfgEnv` instances would be supplemented with the optional value from the chain

This approach has been leveraged before for similar flexibility with 
```rs
fn next_block_base_fee(&self, parent: &Self::Header, target_timestamp: u64) -> Option<u64>
```

which is leveraged in both the mempool and execution.

The default genesis parsing for Eth/OP would **optionally** look for a configuration as below, signalling an override. Although this approach is not fork aware, it gives optionality for those with custom chain specs to modify the genesis shape as they see fit.

```json
  {
    "config": {
      "evmLimits": {
        "maxCodeSize": 24576,
        "maxInitcodeSize": 49152,
        "txGasLimitCap": 30000000
      }
    }
  }
```

Let me know if this sounds reasonable or for alternative suggestions and I can make a PR

### Additional context

_No response_
