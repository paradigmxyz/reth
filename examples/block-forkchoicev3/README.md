# Block ForkchoiceUpdated 示例

这个例子展示了如何使用 Engine API 的 `engine_forkchoiceUpdatedV3` 和 `engine_getPayloadV3` 方法。

## 功能

- 连接到 Engine API (通常在端口 8551)
- 使用 JWT 认证进行安全连接
- 获取最新区块信息
- 构造 ForkchoiceState 和 PayloadAttributes
- 调用 `engine_forkchoiceUpdatedV3` 生成新的 payload
- 使用生成的 payloadId 调用 `engine_getPayloadV3` 获取执行负载

## 运行方式

确保你有 JWT 密钥文件 `jwt.hex` 在项目根目录，然后运行：

```bash
cargo run -p block-forkchoiceUpdatedV4
```

## 注意事项

- **必需**: 需要确保有一个 reth 节点在 `http://localhost:8551` 运行，并且启用了 Engine API
- **必需**: 需要 `jwt.hex` 文件包含正确的 JWT 密钥（十六进制格式）
- Engine API 需要 JWT 认证，这个例子包含了完整的认证实现
- 这是一个演示例子，实际使用时需要设置正确的 fee_recipient 地址和其他参数

## JWT 密钥

项目根目录需要有 `jwt.hex` 文件，内容应该是32字节的十六进制字符串（不包含 0x 前缀）。

## 依赖

主要使用了以下库：
- `alloy-rpc-types-engine`: Engine API 类型定义  
- `alloy-primitives`: 基础类型 (Address, B256 等)
- `reqwest`: HTTP 客户端
- `jsonwebtoken`: JWT 认证
- `hex`: 十六进制编码/解码 