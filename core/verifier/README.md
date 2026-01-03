# Verifier Service

## 概述

验证器服务（Verifier Service）是去中心化GPU算力共享平台的核心安全组件，负责验证多个Provider对同一任务的计算结果，通过简化的BFT（Byzantine Fault Tolerant）共识算法确保结果的正确性。

## 核心功能

### 1. 结果收集（Result Collection）
- 接收来自多个Provider对同一任务的计算结果
- 验证结果格式和完整性
- 跟踪每个Provider的提交状态
- **增强验证**：基于数据模型验证资源使用量、成本计算、时间戳等

### 2. 共识算法（Consensus Algorithm）
- 实现简化的BFT多数派算法
- 要求至少2/3的Provider返回一致结果才认为正确
- 能够容忍少于1/3的恶意节点

### 3. 结果裁定（Result Adjudication）
- 当达到共识时，裁定最终正确结果
- 识别提交错误结果的恶意Provider
- 触发支付机制，向诚实Provider支付报酬

### 4. 惩罚机制（Slashing）
- **多维度严重程度评判**：基于共谋检测、违规频率、结果模式、历史记录等维度自动评估违规严重程度
- **违规类型识别**：支持识别多种违规类型：
  - **结果错误（WrongResult）**：提交错误执行结果
  - **资源夸大（ResourceInflation）**：夸大资源使用量以获取更多报酬
  - **成本欺诈（CostFraud）**：操纵成本计算
  - **时间戳异常（TimestampAnomaly）**：时间戳篡改或异常
  - **格式违规（FormatViolation）**：结果格式不符合要求
  - **多重违规（Multiple）**：同时存在多种违规类型
- **按比例惩罚**：根据Provider的质押金额按比例扣除，更公平合理
- **严重程度分级**：
  - **轻微（Minor）**：0.5% 质押扣除 - 单次错误，可能是硬件故障
  - **中等（Moderate）**：1% 质押扣除 - 重复错误或明显异常
  - **严重（Severe）**：5% 质押扣除 - 共谋攻击或系统性恶意行为
- **递增惩罚**：重复违规的惩罚会递增（1.5倍递增）
- **历史记录跟踪**：记录每个Provider的违规历史和违规时间

## 架构设计

```
┌─────────────────────────────────────────┐
│         Verifier Service                │
├─────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐    │
│  │   Result     │  │  Consensus  │    │
│  │  Collector   │→ │   Engine    │    │
│  └──────────────┘  └──────────────┘    │
│         │                  │           │
│         └────────┬──────────┘           │
│                  │                      │
│         ┌────────▼──────────┐          │
│         │  Slashing Module  │          │
│         │  + Severity      │          │
│         │    Evaluator      │          │
│         └───────────────────┘          │
└─────────────────────────────────────────┘
```

### 核心模块

1. **ResultCollector**: 收集和管理来自多个Provider的结果，生成违规上下文
2. **ConsensusEngine**: 实现BFT共识算法
3. **ResultValidator**: 结果验证器，验证资源使用、成本计算、时间戳等
4. **VerifierService**: 主服务，协调各个模块
5. **SeverityEvaluator**: 多维度严重程度评判器
6. **Slashing**: 惩罚机制实现（支持按比例扣除和严重程度分级）

## 使用方法

### 基本使用

```rust
use ya_verifier::{VerifierService, RegisterTask, SubmitResult, WaitForVerification};

// 创建验证器服务
let service = VerifierService::new(None);
let addr = service.start();

// 注册任务
addr.send(RegisterTask {
    task_id: "task1".to_string(),
    batch_id: "batch1".to_string(),
    expected_providers: 5,
    timeout: Some(Duration::seconds(60)),
}).await?;

// 提交Provider结果
addr.send(SubmitResult {
    result: ProviderResult {
        provider_id: node_id,
        task_id: "task1".to_string(),
        batch_id: "batch1".to_string(),
        results: execution_results,
        timestamp: Utc::now(),
    },
}).await?;

// 等待验证完成
let verification_result = addr.send(WaitForVerification {
    task_id: "task1".to_string(),
    batch_id: "batch1".to_string(),
    timeout: Some(Duration::seconds(60)),
}).await?;
```

### 共识算法

共识算法使用简化的BFT协议：
- 需要至少 `floor(2/3 * N) + 1` 个Provider同意才能达成共识
- 其中N是总Provider数量
- 例如：5个Provider需要4个同意，6个Provider需要5个同意

### 严重程度评判

验证器使用多维度评分系统自动评判违规严重程度：

#### 评判维度（权重）

1. **共谋检测（40%）**
   - 多个节点提交相同错误结果 → 严重
   - 单个节点错误 → 轻微

2. **违规频率（25%）**
   - 1小时内重复违规3次以上 → 严重
   - 首次违规 → 轻微

3. **结果一致性模式和违规类型（20%）**
   - 多个节点结果完全一致但错误 → 共谋模式（严重）
   - 每个节点结果都不同 → 随机错误（中等）
   - 资源夸大2倍以上 → 严重
   - 成本欺诈20%以上 → 严重
   - 时间戳异常超过1小时 → 严重
   - 格式违规 → 中等

4. **历史记录（15%）**
   - 历史违规10次以上 → 严重
   - 首次违规 → 轻微

#### 评判结果

- **总分 ≥ 0.7** → 严重（Severe）：5% 质押扣除
- **总分 ≥ 0.4** → 中等（Moderate）：1% 质押扣除
- **总分 < 0.4** → 轻微（Minor）：0.5% 质押扣除

### 惩罚计算

#### 按比例扣除（推荐模式）

```rust
// 示例：Provider质押1000代币，首次违规（中等严重程度）
penalty = 1000 * 1% = 10代币

// 重复违规（惩罚递增1.5倍）
penalty = 1000 * 1.5% = 15代币
```

#### 惩罚模式

- **Proportional（按比例）**：根据质押金额按比例扣除（推荐）
- **FixedAmount（固定金额）**：固定金额惩罚（向后兼容）
- **Hybrid（混合）**：取固定金额和按比例中的较大值

## 攻击测试

验证器包含完整的攻击测试套件，用于验证系统在各种恶意场景下的鲁棒性：

### 1. 节点共谋攻击（Collusion Attack）
测试多个Provider串通提交相同错误结果的情况。

### 2. 随机错误攻击（Random Error Attack）
测试Provider因硬件故障等原因返回随机错误结果的情况。

### 3. 诚实节点不足（Insufficient Honest Providers）
测试当诚实Provider少于2/3时系统的行为。

### 4. 惩罚机制测试（Slashing Mechanism with Severity）
测试重复违规的递增惩罚机制和严重程度评判系统。

### 5. 资源夸大攻击（Resource Inflation Attack）
测试Provider夸大资源使用量（CPU、GPU、内存等）以获取更多报酬的情况。验证器会检测资源使用量异常，容忍度默认10%。

### 6. 成本欺诈攻击（Cost Fraud Attack）
测试Provider操纵成本计算以多收费的情况。验证器会检测成本计算异常，容忍度默认5%。

### 7. 时间戳篡改攻击（Timestamp Manipulation Attack）
测试Provider篡改时间戳以逃避检测或创建不一致的情况。验证器会检测时间戳异常，最大允许时间差默认5分钟。

### 运行攻击测试

```rust
use ya_verifier::attack_tests::{run_all_attack_tests, generate_test_report};

let results = run_all_attack_tests().await;
let report = generate_test_report(&results);
println!("{}", report);
```

## 前置条件

验证器需要以下前置条件：

1. **多Provider冗余执行**: 任务必须被分发给多个Provider同时执行
2. **结果格式统一**: 所有Provider必须返回相同格式的结果
3. **网络通信**: 需要能够接收来自多个Provider的结果

## 依赖关系

验证器依赖以下模块：
- `ya-core-model`: 核心数据模型
- `ya-client-model`: 客户端模型（用于ExeScriptCommandResult）
- `ya-service-bus`: 服务总线（用于消息传递）
- `actix`: Actor框架

## 性能考虑

- 结果收集是异步的，不会阻塞其他操作
- 共识算法的时间复杂度为O(N)，其中N是Provider数量
- 支持超时机制，避免无限等待

## 安全性

- 使用SHA-256对结果进行哈希，确保完整性
- 使用BFT算法，能够容忍最多1/3的恶意节点
- **多维度严重程度评判**：自动识别共谋攻击和系统性恶意行为
- **按比例惩罚**：公平的惩罚机制，大额质押者损失更大，威慑力更强
- **递增惩罚**：重复违规的惩罚递增，防止惯犯
- **历史跟踪**：记录违规历史，支持长期行为分析

## 严重程度评判示例

### 示例1：共谋攻击
```
场景：3个Provider共谋，提交相同错误结果（共5个Provider）
评判：
- 共谋检测：3/5 = 60% > 30% → 1.0分（40%权重）
- 违规频率：首次违规 → 0.2分（25%权重）
- 结果模式：共谋模式 → 0.8分（20%权重）
- 历史记录：首次违规 → 0.2分（15%权重）

总分 = 1.0*0.4 + 0.2*0.25 + 0.8*0.2 + 0.2*0.15 = 0.67
结果：Severe（严重）- 扣除5%质押
```

### 示例2：随机错误
```
场景：1个Provider提交错误结果（共5个Provider）
评判：
- 共谋检测：单个节点 → 0.2分
- 违规频率：首次违规 → 0.2分
- 结果模式：随机错误 → 0.3分
- 历史记录：首次违规 → 0.2分

总分 = 0.2*0.4 + 0.2*0.25 + 0.3*0.2 + 0.2*0.15 = 0.22
结果：Minor（轻微）- 扣除0.5%质押
```

## 未来改进

- [x] ✅ 多维度严重程度评判系统
- [x] ✅ 按比例惩罚机制
- [ ] 支持更复杂的共识算法（如PBFT）
- [ ] 集成智能合约，实现自动支付和惩罚
- [ ] 添加结果缓存机制
- [ ] 支持结果的部分验证
- [ ] 时间衰减机制（违规后时间越长，惩罚减轻）

