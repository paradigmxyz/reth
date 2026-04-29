# Reject Disabled Mantle MetaTx & Unprotected Legacy Transactions Technical Spec

## 背景

本分支 `fix/reject-mantle-metatx` 让 reth 的 txpool 对两类无效交易的拒绝行为
与 op-geth 对齐：

### 1. Mantle MetaTx（已废弃的 gas sponsorship）

Mantle MetaTx 是 Mantle 历史上的 gas sponsorship 机制。外层交易通过交易
`input` 携带 32 字节前缀 `MantleMetaTxPrefix`，后面跟随 RLP 编码的
MetaTx 参数。op-geth 已通过 `core/types/meta_transaction.go` 中的
`MetaTxCheck` / `ErrMetaTxDisabled` 在公开入口拒绝该类交易，错误文案为：

```text
meta tx is disabled
```

### 2. Unprotected Legacy Transactions（非 EIP-155 重放保护）

Legacy（type 0）交易可以用两种方式签名：
- **EIP-155 签名**（v >= 35）：签名中编码了 chain_id，有重放保护
- **非 EIP-155 签名**（v=27/28）：签名中无 chain_id，可在任意链上重放

op-geth 默认在 RPC 层拒绝非 EIP-155 签名的 legacy 交易
（`AllowUnprotectedTxs=false`，`internal/ethapi/api.go:SubmitTransaction`），
错误文案为：

```text
only replay-protected (EIP-155) transactions allowed over RPC
```

reth 在此之前没有等价的防护。base 层的 `EthTransactionValidator` 中 chain_id
检查使用 `if let Some(chain_id) = transaction.chain_id()`，当 `chain_id()` 返回
`None`（非 EIP-155）时整个检查被跳过，交易无阻碍进入 pool。

本分支在 `OpTransactionValidator` 的 txpool 验证层补上这个缺口，同时覆盖 RPC
和 P2P 两个入口路径（比 op-geth 仅在 RPC 层拦截更严格）。

### 通用说明

由于 reth 仅作为 Verifier 运行，Sequencer 侧（op-geth）已在所有入口全面拒绝
这两类交易，因此 reth 侧只需在 txpool 拒绝即可防止无效交易占用本地资源。
RPC forward、payload builder、engine newPayload 等路径不需要额外防御。

当前分支相对 `main` 的 diff 覆盖以下行为面：

- MetaTx prefix 常量和识别函数。
- txpool 本地验证（MetaTx + Unprotected legacy tx）。
- Mantle receipt 测试时间戳修正（独立修复）。
- `clippy.toml` 移除 `allow-collapsible-match = false` 配置项（独立修复）。

## 变更文件清单

| 文件 | 改动 |
| --- | --- |
| `clippy.toml` | 移除 `allow-collapsible-match = false` 安全注释和配置 |
| `crates/mantle-hardforks/src/lib.rs` | 新增 `MANTLE_META_TX_PREFIX` 常量、`is_mantle_meta_tx` 函数、6 个单元测试 |
| `crates/optimism/rpc/src/eth/receipt.rs` | receipt 测试时间戳从 OP mainnet 常量改为 `MANTLE_MAINNET_ARSIA_TIMESTAMP`，测试名 `blob_gas_used_not_included_in_receipt_post_isthmus` 改为 `blob_gas_used_not_included_in_receipt_pre_jovian` |
| `crates/optimism/txpool/src/error.rs` | 新增 `MetaTxDisabled` 和 `UnprotectedTxDisabled` 错误类型 |
| `crates/optimism/txpool/src/lib.rs` | 导出 `MetaTxDisabled` 和 `UnprotectedTxDisabled` |
| `crates/optimism/txpool/src/transaction.rs` | 2 个 MetaTx 测试 + 3 个 EIP-155 测试 + `pooled_legacy_tx` 辅助函数 |
| `crates/optimism/txpool/src/validator.rs` | `validate_one_with_state` 中拒绝 EIP-4844 后依次拒绝 unprotected legacy tx 和 MetaTx |

## 目标

1. 使用统一的 `MANTLE_META_TX_PREFIX` 和 `is_mantle_meta_tx(input)`，避免不同入口重复实现。
2. 与 op-geth `MetaTxCheck` 的边界保持一致：只有 `len(input) > 32` 且前 32 字节等于 prefix 时才拒绝；恰好 32 字节 prefix 不拒绝。
3. 在用户交易进入本地 txpool 时拒绝 MetaTx，防止无效交易占用 pool 资源。
4. 拒绝非 EIP-155（unprotected）legacy 交易，与 op-geth `AllowUnprotectedTxs=false` 默认行为对齐，防止跨链重放攻击。
5. 补充单元测试，覆盖 MetaTx 拒绝路径、EIP-155 执行路径和边界输入。

## 非目标

- 不恢复、解析或执行 MetaTx inner transaction。
- 不实现 sponsor 签名校验。
- 不修改交易编码、receipt schema、engine API schema。
- 不引入数据库迁移或链数据格式迁移。
- 不在本次改动中引入新的 MantleEverest hardfork gating。当前 reth 侧没有单独的 Everest hardfork 模型，因此拒绝逻辑在 txpool 内按 prefix 统一生效。若同一 binary 服务非 Mantle OP 链，需要在合入前确认该全局行为是否可接受，或后续按 chain spec 收窄。
- 不在 RPC（estimateGas、sendRawTransaction、conditional、preconf）层拒绝。reth 作为 Verifier，RPC forward 到 Sequencer 后由 Sequencer 拒绝。
- 不在 payload builder 或 engine newPayload 路径拒绝。Verifier 不出块，Sequencer 不会打包 MetaTx。

## 范围决策说明

reth 仅作为 Verifier 部署，Sequencer 使用 op-geth。在此前提下：

| 路径 | Verifier 视角 | 是否需要 reth 侧拦截 |
| --- | --- | --- |
| Txpool | 防止无效 tx 占用本地 pool 资源 | 需要 |
| Payload Builder | Verifier 不出块 | 不需要 |
| Engine newPayload | Sequencer 不会打包 MetaTx，payload 中不会有 | 不需要 |
| eth_sendRawTransaction forward | Forward 到 Sequencer，Sequencer 会拒绝 | 不需要 |
| eth_sendRawTransactionConditional | 同上 | 不需要 |
| eth_sendRawTransactionWithPreconf | 同上 | 不需要 |
| eth_estimateGas | 纯模拟，不影响链状态 | 不需要 |

若后续 reth 需要作为 Sequencer 运行，应重新评估并扩展拦截范围到 RPC、payload builder 和 engine 路径。

## op-geth 对齐矩阵

| 场景 | op-geth 行为 | reth 方案 |
| --- | --- | --- |
| MetaTx Prefix 识别 | `MetaTxCheck(txData)`：`len <= 32` 放行，前 32 字节匹配则 `ErrMetaTxDisabled` | `reth-mantle-forks::is_mantle_meta_tx` 使用同样长度和前缀边界 |
| MetaTx Txpool | `core/txpool/validation.go` 调用 `types.MetaTxCheck(tx.Data())` | `OpTransactionValidator` 返回 bad transaction `MetaTxDisabled` |
| Unprotected legacy tx (RPC) | `internal/ethapi/api.go:SubmitTransaction` 检查 `!tx.Protected()` 时拒绝（默认 `AllowUnprotectedTxs=false`） | `OpTransactionValidator` 检查 `ty() == LEGACY && chain_id().is_none()` 时拒绝 |
| Unprotected legacy tx (P2P) | op-geth RPC 层拦截，P2P 层 signer 接受（`HomesteadSigner`）| reth 在 txpool 验证层拦截，同时覆盖 RPC 和 P2P（比 op-geth 更严格） |
| EIP-155 legacy tx (正确 chain_id) | 接受 | 接受（chain_id 检查在 `EthTransactionValidator` 中通过） |
| EIP-155 legacy tx (错误 chain_id) | `ErrInvalidChainId` | `ChainIdMismatch` |
| 其他入口 | op-geth 在 estimateGas、sendRawTransaction 等入口均拒绝 MetaTx | reth 作为 Verifier 不需要覆盖；Sequencer（op-geth）已全面拦截 |

## 详细设计

### 1. MetaTx 识别中心化

在 `crates/mantle-hardforks/src/lib.rs` 新增：

```rust
pub const MANTLE_META_TX_PREFIX: [u8; 32] = [/* 14 zero bytes + "MantleMetaTxPrefix" ASCII */];

#[inline]
pub fn is_mantle_meta_tx(input: &[u8]) -> bool {
    input.len() > 32 && input[..32] == MANTLE_META_TX_PREFIX
}
```

该函数是所有入口唯一的 prefix 判定来源。它只判断 calldata 前缀，不尝试 decode inner MetaTx。

### 2. Txpool 拒绝

`crates/optimism/txpool/src/validator.rs` 在 `validate_one_with_state` 中，interop 校验之前执行：

1. 保留已有 EIP-4844 拒绝。
2. 调用 `is_mantle_meta_tx(transaction.input())`。
3. 命中后使用 `trace!` 记录日志并返回 `TransactionValidationOutcome::Invalid`。
4. 错误为 `InvalidPoolTransactionError::other(MetaTxDisabled)`。

`MetaTxDisabled` 定义在 `crates/optimism/txpool/src/error.rs`，实现 `PoolTransactionError`，`is_bad_transaction()` 返回 `true`，表示该交易确定无效，不应重试。通过 `crates/optimism/txpool/src/lib.rs` 公开导出。

### 3. Unprotected Legacy Tx 拒绝

`crates/optimism/txpool/src/validator.rs` 在 `validate_one_with_state` 中，EIP-4844 拒绝之后、MetaTx 拒绝之前执行：

```rust
if transaction.ty() == LEGACY_TX_TYPE_ID && transaction.chain_id().is_none() {
    return TransactionValidationOutcome::Invalid(
        transaction,
        InvalidPoolTransactionError::other(UnprotectedTxDisabled),
    );
}
```

检查条件：
- `ty() == LEGACY_TX_TYPE_ID`：仅针对 type 0（legacy）交易
- `chain_id().is_none()`：仅针对未编码 chain_id 的签名（v=27/28，非 EIP-155）

以下交易不受影响：
- EIP-155 签名的 legacy 交易（`chain_id = Some(_)`）
- EIP-2930（type 1）、EIP-1559（type 2）、EIP-7702（type 4）等 typed 交易（始终携带 chain_id）

`UnprotectedTxDisabled` 定义在 `crates/optimism/txpool/src/error.rs`，实现 `PoolTransactionError`，`is_bad_transaction()` 返回 `true`。错误文案为 `"only replay-protected (EIP-155) transactions allowed"`。

**与 op-geth 的差异：** op-geth 在 RPC 层拦截（`AllowUnprotectedTxs` flag），P2P gossip 绕过。reth 在 txpool 验证层拦截，RPC 和 P2P 均覆盖。这更安全，但目前没有提供 `--rpc.allow-unprotected-txs` 等开关。若后续需要兼容，可在 `OpTransactionValidator` 中增加配置项。

### 4. Receipt 测试时间戳修正

`crates/optimism/rpc/src/eth/receipt.rs` 的 Mantle receipt 测试做了以下修正：

- 时间戳从 `OP_MAINNET_JOVIAN_TIMESTAMP` / `OP_MAINNET_ISTHMUS_TIMESTAMP` 改为 `MANTLE_MAINNET_ARSIA_TIMESTAMP` 及其前一秒。
- 测试名 `blob_gas_used_not_included_in_receipt_post_isthmus` 改为 `blob_gas_used_not_included_in_receipt_pre_jovian`。

该修改不改变生产逻辑，只修正测试语义使其使用 Mantle 自身的 hardfork 时间戳。

### 5. clippy.toml 变更

移除 `allow-collapsible-match = false` 配置项及其安全注释。该配置原先用于防止 clippy 的 `collapsible_match` lint 将 if-inside-match 转换为 match guard 导致语义变化。

## 错误语义

| 入口 | 场景 | 行为 | 错误 |
| --- | --- | --- | --- |
| Txpool | MetaTx | `TransactionValidationOutcome::Invalid`，bad transaction | `meta tx is disabled` |
| Txpool | Unprotected legacy tx | `TransactionValidationOutcome::Invalid`，bad transaction | `only replay-protected (EIP-155) transactions allowed` |

## 测试设计

### 单元测试

拒绝测试输入统一为 `prefix + 0xF8`（33 字节）。边界测试覆盖空 input、恰好 32 字节 prefix、错误 prefix。

| 模块 | 案例 | 预期 |
| --- | --- | --- |
| mantle-hardforks | 常量尾 18 字节 == ASCII "MantleMetaTxPrefix" | 通过 |
| | prefix + payload（33B） | 识别为 MetaTx |
| | 仅 prefix（32B） | 不识别（与 op-geth 一致） |
| | 空 / 10B / 32B 全零 / 33B 全零 / 篡改 prefix | 均不识别 |
| txpool | MetaTx EIP-1559 tx 经 validator | Invalid + MetaTxDisabled, bad_tx=true |
| | 32B prefix tx 经 validator | 不触发 MetaTxDisabled |
| txpool | Unprotected legacy tx（chain_id=None）经 validator | Invalid + UnprotectedTxDisabled, bad_tx=true |
| | EIP-155 legacy tx（chain_id=Some(10)）经 validator | 不触发 UnprotectedTxDisabled |
| | EIP-1559 tx 经 validator | 不触发 UnprotectedTxDisabled |
| rpc/receipt | Arsia 时间戳 receipt（3 个） | 字段存在性与 hardfork 时间戳匹配 |

### rde-v3 冒烟测试

使用 `task up-all-reth` 启动 op-reth/Mantle 栈（含 op-node、op-reth sequencer、geth-rpc）。

| 案例 | 操作 | 预期 |
| --- | --- | --- |
| sendRawTx + MetaTx 入池拒绝 | 签名 MetaTx input 的 EIP-1559 tx，`cast send` | txpool 拒绝，"meta tx is disabled" |
| 普通交易不受影响 | 签名正常 EIP-1559 tx（空 input 或非 prefix），`cast send` | 正常上链 |
| 32 字节 prefix 不拒绝 | input = 恰好 32 字节 prefix | 正常入池 |
| Unprotected legacy tx 拒绝 | 构造 v=27/28 无 chain_id 的 legacy tx，`cast publish` | txpool 拒绝，"only replay-protected (EIP-155) transactions allowed" |
| EIP-155 legacy tx 正常 | 带正确 chain_id 的 legacy tx，`cast send --legacy` | 正常上链 |
| 错误 chain_id legacy tx 拒绝 | 带 chain_id=999 的 legacy tx | 拒绝，"invalid chain ID" |
| EIP-1559 tx 不受影响 | 标准 EIP-1559 tx，`cast send` | 正常上链 |

**冒烟测试结果（2026-04-29）：**

| 测试 | op-reth (localhost:9545) | op-geth (127.0.0.1:19545) | 一致 |
| --- | --- | --- | --- |
| EIP-155 legacy tx (chain_id=1337) | ✅ 接受，上链 | ✅ 接受 | ✅ |
| Unprotected legacy tx (v=27/28) | ✅ 拒绝 | ✅ 拒绝 | ✅ |
| EIP-1559 (type 2) tx | ✅ 接受，上链 | ✅ 接受 | ✅ |
| EIP-155 legacy tx (chain_id=999) | ✅ 拒绝 | ✅ 拒绝 | ✅ |

### 建议本地验证命令

```bash
cargo test -p reth-mantle-forks meta_tx
cargo test -p reth-optimism-txpool --lib --features reth-optimism-primitives/serde,reth-optimism-primitives/reth-codec validate_rejects_mantle_meta_tx_as_bad_transaction
cargo test -p reth-optimism-txpool --lib --features reth-optimism-primitives/serde,reth-optimism-primitives/reth-codec validate_does_not_reject_exact_meta_tx_prefix_without_payload
cargo test -p reth-optimism-rpc blob_gas_used
cargo +nightly fmt --all -- --check
git diff --check
```

完整合入前建议再跑相关 crate 全量测试或 CI。`reth-optimism-txpool` 当前本地精确测试需要显式启用 `reth-optimism-primitives/serde,reth-optimism-primitives/reth-codec`，避免落入默认 feature 下的既有 trait bound 问题。

## 部署计划

### 合入前

1. 确认部署范围：如果该 binary 只用于 Mantle Verifier，当前 prefix-based txpool 拒绝可接受；如果复用到其他 OP 链，评估是否需要 chain gating。
2. 跑上述精确测试和 `cargo +nightly fmt --all -- --check`。

### Staging / Testnet

1. 部署到 Mantle Sepolia 或内部 devnet。
2. 构造 `input = MANTLE_META_TX_PREFIX || 0xf8` 的交易：
   - `eth_sendRawTransaction` 应在 txpool 阶段拒绝，返回 "meta tx is disabled"。
3. 普通交易、空 input、恰好 32 字节 prefix、错误 prefix 交易应保持原行为。
4. 手工 smoke 优先用 `task up-all-reth` 验证 op-reth 分支行为；`task up-all` 仅用于 op-geth 对照。

### Production

1. 滚动升级节点二进制，不需要数据库迁移。
2. 先升级 canary 节点，观察：
   - 同步是否正常。
   - txpool 是否误拒绝普通交易。
3. 扩大到全量节点。

## 回滚计划

本次改动无状态迁移，回滚方式是重新部署上一版本二进制。

回滚触发条件：

- 普通交易被误判为 MetaTx。
- 非 Mantle OP 链因相同 prefix calldata 出现不可接受兼容性问题。

回滚后保留原始交易 input 和节点日志，用于判断是否需要增加 chain gating。

## 风险与缓解

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| 当前实现无 chain gating | 非 Mantle 链上同 prefix calldata 会被 txpool 拒绝 | 合入前确认部署范围；必要时按 chain spec 收窄 |
| 仅 txpool 拦截，RPC/payload/engine 路径未覆盖 | 若 reth 后续作为 Sequencer 运行，MetaTx 可能绕过 txpool 进入出块路径 | 当前仅 Verifier 部署，Sequencer 为 op-geth 已全面拦截；若 reth 升级为 Sequencer 需重新评估 |
| 错误文案被客户端依赖 | 文案变更会影响调用方断言 | 固定为 op-geth 文案 `meta tx is disabled` 和 `only replay-protected (EIP-155) transactions allowed` |
| Unprotected tx 拒绝无开关 | 无法像 op-geth 那样用 `--rpc.allow-unprotected-txs` 临时放行 | 当前需求无需放行；后续可在 OpTransactionValidator 增加配置项 |
| reth 比 op-geth 更严格（P2P 也拦截） | P2P gossip 来的 unprotected tx 也被拒绝，op-geth 不会 | 更安全的默认行为；若需对齐 op-geth 可将检查移到 RPC 层 |

## 验收标准

1. `MANTLE_META_TX_PREFIX || payload` 在 txpool 入口被拒绝。
2. 边界输入不被误拒绝：空 input、短 input、恰好 32 字节 prefix、错误 prefix。
3. `MetaTxDisabled` 在 txpool 中被标记为 bad transaction。
4. 非 EIP-155（v=27/28）legacy 交易在 txpool 入口被拒绝。
5. EIP-155 签名的 legacy 交易（正确 chain_id）不被拒绝。
6. EIP-1559 等 typed 交易不受 unprotected tx 检查影响。
7. `UnprotectedTxDisabled` 在 txpool 中被标记为 bad transaction。
8. Mantle receipt 测试使用 Mantle hardfork 时间戳。
9. 无数据库迁移，支持滚动升级和二进制回滚。
