# RPC发送交易示例

这个例子展示了如何在reth中通过RPC发送交易的各种方法。

## 功能特性

- **连接到任何以太坊RPC节点**: 兼容reth、geth、erigon等
- **多种发送方式**: 演示了两种不同的交易发送方法
- **支持Legacy和EIP-1559交易**: 自动检测并使用合适的交易类型
- **详细日志**: 显示交易的详细信息和确认状态

## 可用的程序

### 1. main - 完整演示
展示两种发送交易的方法：
- 原始交易签名和发送
- 使用alloy Provider的高级功能

### 2. simple-client-simple - 超简单示例
最小化的示例，易于理解和修改

## 发送交易的方法

### 1. 原始交易 (eth_sendRawTransaction)
手动创建、签名和发送原始交易字节码：
- 使用私钥手动签名
- 发送已编码的交易数据
- 适用于离线签名场景

### 2. Alloy Provider高级功能
使用alloy的Provider进行现代化交易管理：
- 支持EIP-1559交易类型
- 自动管理nonce和gas费用
- 内置钱包集成

## 使用方法

### 主要演示程序
```bash
cargo run -p rpc-send-transaction -- --rpc-url http://localhost:8545
```

### 简单演示程序
```bash
cargo run -p rpc-send-transaction --bin simple-client-simple -- --rpc-url http://localhost:8545
```

### 自定义参数
```bash
cargo run -p rpc-send-transaction -- \
  --rpc-url http://localhost:8545 \
  --to 0x742d35cc6065c8532b5566d49e529c0e8bf1935b \
  --amount 0.5 \
  --trace
```

## 命令行参数

- `--rpc-url`: RPC节点地址 (默认: http://localhost:8545)
- `--to`: 接收者地址 (默认: 0x70997970C51812dc3A010C7d01b50e0d17dc79C8)
- `--amount`: 发送的ETH数量 (默认: 0.1)
- `--trace`: 启用详细日志

## 测试账户

例子中使用了几个预设的测试私钥，这些是公开的测试网络账户：

1. **发送者1**: `0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80`
   - 地址: `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266`
   
2. **发送者2**: `0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d`
   - 地址: `0x70997970C51812dc3A010C7d01b50e0d17dc79C8`

⚠️ **警告**: 这些是测试私钥，切勿在主网使用！

## 输出示例

```
INFO 启动开发节点...
INFO 开发节点已启动，RPC地址: http://localhost:8545
INFO === RPC发送交易演示 ===
INFO 1. 发送原始交易 (eth_sendRawTransaction)
INFO 创建并签名交易...
INFO 发送者地址: 0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266
INFO 当前nonce: 0
INFO Gas价格: 1000000000
INFO 发送原始交易...
INFO 交易已发送，哈希: 0x1234...
INFO 新区块 #1: 0x5678...
INFO   包含 1 笔交易
INFO 交易已确认，gas used: 21000
```

## 技术细节

### RPC方法使用
- `eth_getTransactionCount`: 获取账户nonce
- `eth_gasPrice`: 获取当前gas价格  
- `eth_sendRawTransaction`: 发送原始交易
- `eth_sendTransaction`: 发送交易请求
- `eth_getTransactionReceipt`: 获取交易回执

### 交易类型
- **Legacy交易**: 传统的以太坊交易格式
- **EIP-1559交易**: 现代的交易格式，支持动态费用

### 错误处理
程序包含完善的错误处理，会显示：
- 连接错误
- 签名错误  
- 交易发送失败
- 确认超时等

## 扩展用法

这个例子可以作为基础，扩展为：
- 批量交易发送工具
- 交易监控服务
- 自动化测试框架
- Gas价格优化工具

## 相关文档

- [Reth RPC API文档](https://paradigmxyz.github.io/reth/)
- [Alloy Provider文档](https://alloy.rs/providers/providers.html)
- [以太坊JSON-RPC规范](https://ethereum.org/en/developers/docs/apis/json-rpc/) 