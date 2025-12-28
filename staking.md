# 第一章节 本地质押

## 1. 概述

本章节介绍 Golem 网络的本地质押（Local Staking）机制。目前，Staking 模块采用**本地 SQLite 数据库**进行状态管理，这是一种轻量级的实现方案，适用于单节点环境下的快速验证与开发。

该机制的核心目标是为 Provider 提供一套完整的经济激励模型，包括质押（Stake）、奖励（Reward）、惩罚（Slash）和提现（Withdraw）等功能，确保网络参与者能够通过正确的经济行为获得收益，同时对恶意行为进行约束。

## 2. 核心组件与数据模型

### 2.1 状态存储 (StakingState)

质押状态由 `StakingState` 结构体管理，底层使用 SQLite 连接池 (`r2d2_sqlite`)。

*   **数据存储位置**: `staking.db` 文件（通常位于节点数据目录）。
*   **并发控制**: 通过 `r2d2` 连接池支持并发访问。

### 2.2 数据表设计

系统主要维护两张表：

1.  **`providers` 表**: 存储 Provider 的当前账户状态。
    *   `provider_id` (TEXT PRIMARY KEY): 唯一标识符。
    *   `stake` (REAL): 当前质押的本金余额。
    *   `rewards` (REAL): 累计获得的奖励余额（可提现）。
    *   `slashed` (REAL): 累计被惩罚扣除的总额（统计用）。
    *   `updated_at` (TEXT): 最后更新时间戳。

2.  **`events` 表**: 记录所有资金变动的流水日志。
    *   `id` (INTEGER PRIMARY KEY): 自增流水号。
    *   `provider_id` (TEXT): 关联 Provider。
    *   `event_type` (TEXT): 事件类型 (`register`, `stake`, `reward`, `slash`, `withdraw`)。
    *   `amount` (REAL): 变动金额。
    *   `memo` (TEXT): 备注（如惩罚原因）。
    *   `ts` (TEXT): 事件发生时间。

### 2.3 核心数据结构

```rust
pub struct ProviderRecord {
    pub provider_id: String,
    pub stake: f64,
    pub rewards: f64,
    pub slashed: f64,
    pub updated_at: String,
}
```

## 3. 业务流程与逻辑

### 3.1 注册 (Register)
*   **操作**: `register(pid, stake)`
*   **逻辑**: 
    *   在 `providers` 表中创建或更新记录。
    *   设置初始 `stake` 金额。
    *   记录 `register` 事件。

### 3.2 追加质押 (Stake)
*   **操作**: `stake(pid, amount)`
*   **逻辑**:
    *   增加 `stake` 余额。
    *   记录 `stake` 事件。
*   **约束**: 金额必须为正数。

### 3.3 奖励发放 (Reward)
*   **操作**: `reward(pid, amount)`
*   **逻辑**:
    *   增加 `rewards` 余额。
    *   记录 `reward` 事件。
*   **特点**: 奖励与本金分离，直接进入可提现账户。

### 3.4 惩罚机制 (Slash)
*   **操作**: `slash(pid, amount, reason)`
*   **逻辑**:
    1.  优先扣除 `stake` 本金。
    2.  如果本金不足，扣除 `rewards` 余额。
    3.  如果两者之和仍不足，报错 `insufficient funds`。
    4.  累加 `slashed` 统计字段。
    5.  记录 `slash` 事件及原因。

### 3.5 提现 (Withdraw)
*   **操作**: `withdraw(pid, amount)`
*   **逻辑**:
    *   检查 `rewards` 余额是否充足。
    *   扣除 `rewards` 余额。
    *   记录 `withdraw` 事件。
*   **约束**: 只能提现奖励部分，不能直接提现本金（解除质押需走另外流程，当前版本未展示解质押接口）。

## 4. 测试验证

系统包含完整的集成测试 (`test_staking_api.rs`)，覆盖了上述所有流程的端到端验证：
1.  **Register**: 验证初始状态创建。
2.  **Stake More**: 验证本金增加。
3.  **Reward**: 验证奖励累积。
4.  **Slash**: 验证惩罚扣款顺序（本金 -> 奖励）。
5.  **Withdraw**: 验证奖励提取及余额检查。
6.  **Get Provider**: 验证数据持久化一致性。

## 5. 总结

当前的 Staking 模块是一个**基于本地 SQLite 数据库**的独立实现。它不依赖外部区块链或分布式账本，适合作为：
*   单机节点的内部记账系统。
*   测试环境下的经济模型模拟器。
*   未来分布式共识机制的本地状态缓存层。

---

# 第二章节 去中心化共识模拟 (Chapter 2: Decentralized Consensus Simulation)

## 1. 概述

本章介绍了如何在本地 Staking 模块之上构建一个**去中心化共识与争议解决机制**。该机制通过“承诺-揭示（Commit-Reveal）”协议和“质押挑战（Staking Challenge）”模型，实现了多 Provider 冗余计算的链下共识与仲裁。

## 2. 核心组件升级

在原有 `staking.db` 的基础上，新增了以下表结构以支持共识逻辑：

### 2.1 新增数据表

1.  **`commits` 表**: 记录 Provider 对计算结果的加密承诺。
    *   `task_id` (TEXT): 任务 ID。
    *   `provider_id` (TEXT): Provider ID。
    *   `commitment_hash` (TEXT): 结果哈希 `Hash(Result + Salt)`。
    *   `revealed_result` (TEXT): 揭示后的明文结果。
    *   `salt` (TEXT): 揭示后的盐值。
    *   `status` (TEXT): `committed` | `revealed` | `disputed`。

2.  **`disputes` 表**: 记录争议与仲裁过程。
    *   `id` (INTEGER PRIMARY KEY): 争议 ID。
    *   `task_id` (TEXT): 关联任务。
    *   `accuser_id` (TEXT): 原告（通常为 Requestor）。
    *   `defendant_id` (TEXT): 被告（被挑战的 Provider）。
    *   `evidence` (TEXT): 证据（如结果哈希不匹配证明）。
    *   `status` (TEXT): `pending` | `resolved_guilty` | `resolved_innocent`。

## 3. 共识流程 (The Consensus Flow)

系统实现了完整的 **Commit-Reveal-Challenge** 状态机：

### 3.1 承诺阶段 (Commit)
*   **API**: `POST /staking/commit`
*   **行为**: Provider 提交计算结果的哈希值。
*   **目的**: 防止 Provider 互相抄袭结果，确保独立计算。

### 3.2 揭示阶段 (Reveal)
*   **API**: `POST /staking/reveal`
*   **行为**: Provider 提交结果明文和盐值。
*   **验证**: 系统自动校验 `Hash(Result + Salt) == CommitmentHash`。
*   **目的**: 公开结果以供 Requestor 验证共识。

### 3.3 挑战阶段 (Challenge)
*   **API**: `POST /staking/challenge`
*   **行为**: Requestor 若发现某 Provider 结果与主流共识不符，发起挑战。
*   **机制**: 
    *   创建 `dispute` 记录。
    *   将 Commit 状态标记为 `disputed`。

### 3.4 仲裁与执行 (Resolve)
*   **API**: `POST /staking/resolve`
*   **行为**: 仲裁者（Jury/Admin）裁定被告是否作恶。
*   **结果**:
    *   **Guilty (有罪)**: 调用 `slash()` 扣除被告质押金，并调用 `reward()` 将部分罚金奖励给原告。
    *   **Innocent (无罪)**: 恢复状态（甚至反向惩罚原告，当前 MVP 仅实现惩罚被告）。

## 4. 集成验证

新增集成测试 `core/payment/tests/test_staking_consensus.rs` 模拟了完整的博弈场景：

1.  **Setup**: 注册 3 个 Provider (P1, P2, P3) 和 1 个 Requestor。
2.  **Commit**: P1, P2 提交结果 "42" 的哈希；P3 提交结果 "00" 的哈希。
3.  **Reveal**: 所有节点揭示明文。系统验证哈希一致性。
4.  **Challenge**: Requestor 发现 P3 结果 ("00") 与 P1/P2 ("42") 不一致，发起挑战。
5.  **Resolve**: 模拟仲裁庭判定 P3 有罪。
6.  **Verify**: 
    *   P3 被 Slash 100 GLM。
    *   Requestor 获得 50 GLM 奖励。

该测试证明了在不依赖 Layer 1 智能合约的情况下，仅通过本地状态机和 API 也能实现复杂的 Layer 2 争议解决逻辑。

---

# 第三章节 迈向 Layer 2 (Chapter 3: Towards Layer 2)

## 1. 概述

本章阐述如何将第二章的本地共识机制迁移到 **Layer 2 (Polygon)**，从而实现真正的去中心化。通过智能合约 (`ConsensusStaking.sol`)，我们可以移除对本地 `staking.db` 的依赖，将信任锚点从单节点数据库转移到区块链。

## 2. 智能合约架构

新创建的合约位于 `yagna-staking/contracts/ConsensusStaking.sol`，它完全对应了 Rust 中的业务逻辑，但增加了加密货币（ERC20）的原生支持。

### 2.1 合约接口

*   `register(amount)`: 调用 ERC20 `transferFrom` 锁定代币。
*   `commit(taskId, hash)`: 链上存储哈希，具有不可篡改的时间戳证明。
*   `reveal(taskId, result, salt)`: 链上验证哈希，确保证据的真实性。
*   `slashMalicious(provider, amount, accuser)`: 执行链上资金划转，实现无需信任的惩罚。

## 3. 迁移指南 (Migration Guide)

要从当前的 SQLite 版本迁移到 L2 版本，需要对 Rust 代码进行以下适配：

### 3.1 驱动层替换 (Driver Adapter)

需要创建一个新的 `StakingDriver` trait 实现，使用 `web3` 或 `ethers-rs` 库替换 `rusqlite`。

**旧代码 (SQLite)**:
```rust
pub fn commit(&self, task_id: &str, pid: &str, hash: &str) -> Result<()> {
    conn.execute("INSERT INTO commits ...", params![...])?;
    Ok(())
}
```

**新代码 (Web3/Ethers)**:
```rust
pub async fn commit(&self, task_id: &str, hash: [u8; 32]) -> Result<TxHash> {
    let contract = Contract::new(web3, address, abi);
    let tx = contract.call("commit", (task_id, hash), options).await?;
    Ok(tx)
}
```

### 3.2 异步处理

由于区块链交互是异步且有延迟的（等待区块确认），所有的 Staking API 必须完全异步化，并且需要处理：
*   **Gas 估算**: 动态计算交易费用。
*   **Nonce 管理**: 防止交易冲突。
*   **事件监听**: 通过 WebSocket 监听 `Committed` 和 `Revealed` 事件，而不是轮询数据库。

## 4. 总结

通过引入 Layer 2 智能合约，Yagna 的共识机制将具备：
*   **抗审查性**: 没有任何单点可以删除或修改承诺记录。
*   **经济安全性**: 质押金被合约锁定，作恶惩罚由代码强制执行，而非依赖管理员意愿。
*   **可组合性**: 可以与其他 DeFi 协议（如借贷、保险）集成。
