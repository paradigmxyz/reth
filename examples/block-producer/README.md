# Block Producer Example

这个示例演示如何使用 Engine API 进行区块生产，包括 `engine_newPayload` 和 `engine_forkchoiceUpdated` 调用。

## 🔧 节点配置要求

这个示例需要根据你的 reth 节点配置选择正确的 Engine API 版本：

### Prague 配置 (推荐)
如果你的节点配置了 Prague 硬分叉，你需要：

```bash
# 启动支持 Prague 的 reth 节点
reth node --engine.accept-execution-requests-hash

# 运行示例 (使用 engine_newPayloadV4)
cargo run -p block-producer
```

### Cancun 配置
如果你想演示 `engine_newPayloadV3`，请使用 Cancun 配置：

```bash
# 启动仅支持 Cancun 的 reth 节点 (禁用 Prague)
reth node --dev --engine.legacy

# 运行示例 (使用 engine_newPayloadV3)
cargo run -p block-producer
```

## 🚀 使用说明

1. **准备 JWT 文件**
   ```bash
   # 生成 JWT 密钥
   openssl rand -hex 32 > jwt.hex
   ```

2. **启动 reth 节点**
   ```bash
   # Prague 配置 (推荐)
   reth node --engine.accept-execution-requests-hash --authrpc.jwtsecret jwt.hex
   
   # 或 Cancun 配置 (用于演示 V3)
   reth node --dev --engine.legacy --authrpc.jwtsecret jwt.hex
   ```

3. **运行示例**
   ```bash
   cargo run -p block-producer
   ```

## 📋 功能特性

- ✅ **自动检测硬分叉配置**: 代码会自动检测节点的硬分叉配置
- ✅ **智能 API 版本选择**: 根据节点配置使用 V3 或 V4
- ✅ **详细错误诊断**: 提供具体的配置建议
- ✅ **完整的 Engine API 流程**: 包含 newPayload、forkchoiceUpdated 和 getPayload

## 🔍 输出示例

```
🚀 启动区块生产示例
✅ 找到 JWT 文件: jwt.hex
✅ JWT token 创建成功

📊 获取最新区块信息...
🔍 区块诊断信息:
  requestsHash: "0xe3b0c44..."
  🚨 检测到 Prague 字段！节点配置了 Prague 硬分叉

📤 调用 engine_newPayloadV4...
✅ engine_newPayload 响应: Valid

🔄 调用 engine_forkchoiceUpdated 更新链头...
✅ 获得 payloadId: 0x1234...

📦 调用 engine_getPayload 获取构建的载荷...
✅ 载荷构建成功！

🎉 区块生产示例完成！
```

## ❗ 常见问题

### "missing requests hash" 错误
```
解决方案：你的节点配置了 Prague 硬分叉，需要启动时添加：
--engine.accept-execution-requests-hash
```

### "requests hash cannot be accepted" 错误
```
解决方案：重新启动 reth 时添加标志：
reth node --engine.accept-execution-requests-hash
```

### 想演示 engine_newPayloadV3
```
解决方案：使用 Cancun 配置：
reth node --dev --engine.legacy
```

## 🎯 学习目标

通过这个示例，你将学会：
- 如何构造和提交 ExecutionPayload
- 如何使用 engine_forkchoiceUpdated 更新链状态
- 如何处理不同的硬分叉配置
- 如何诊断和解决 Engine API 错误

## 📚 相关资源

- [Engine API 规范](https://github.com/ethereum/execution-apis/tree/main/src/engine)
- [Reth 引擎 API 文档](https://reth.rs/)
- [Prague 硬分叉特性](https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md) 