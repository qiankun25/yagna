# Verifier Module - 验证器模块

## 概述

验证器模块实现了"多Provider冗余执行 + 局部结果共识"机制，用于验证去中心化GPU算力共享平台中计算结果的正确性。

**注意**: 验证器功能已整合到独立的 `ya-verifier` 模块中。本模块（`ya-activity::requestor`）通过 `ResultCollector` 使用 `ya-verifier` 提供的验证服务。

## 核心组件

### 1. Verifier Service (验证器服务)

验证器功能由独立的 `ya-verifier` 模块提供，实现了完整的BFT（Byzantine Fault Tolerance）共识算法和惩罚机制。

**主要功能：**
- 收集多个Provider对同一任务的计算结果
- 运行简化BFT多数派算法（≥2/3一致结果）
- 识别恶意Provider并触发惩罚机制
- 多维度严重程度评估
- 比例惩罚机制

**使用示例：**
```rust
use ya_verifier::{VerifierService, SlashingConfig};

// 创建验证器服务（Actix Actor）
let verifier_service = VerifierService::new(None);

// 或者使用自定义配置
let slashing_config = SlashingConfig::default();
let verifier_service = VerifierService::new(Some(slashing_config));

// 注册任务
verifier_service.send(RegisterTask {
    task_id: "task-1".to_string(),
    batch_id: "batch-1".to_string(),
    expected_providers: 5,
    timeout: None,
}).await?;

// 提交结果
verifier_service.send(SubmitResult {
    result: provider_result,
}).await?;

// 获取验证状态
let status = verifier_service.send(GetVerificationStatus {
    task_id: "task-1".to_string(),
    batch_id: "batch-1".to_string(),
}).await??;
```

更多详细信息请参考 `core/verifier/README.md`。

### 2. ResultCollector (结果收集器)

`result_collector.rs` 负责从多个Provider异步收集计算结果。

**主要功能：**
- 并发向多个Provider发送执行请求
- 管理超时和错误处理
- 收集结果并调用验证器进行验证

**使用示例：**
```rust
use ya_activity::requestor::result_collector::ResultCollector;

let collector = ResultCollector::new();
let verification_result = collector.collect_and_verify(
    agreements,
    activity_ids,
    batch_id,
    exe_script,
    requestor_id,
).await?;
```

### 3. Attack Tests (攻击实验)

攻击测试功能由 `ya-verifier` 模块提供，包含完整的攻击场景模拟测试。

**支持的攻击场景：**
- **节点共谋 (Collusion)**: 多个Provider串通返回相同的错误结果
- **随机错误 (Random Errors)**: Provider因硬件故障等原因返回随机错误结果
- **超时攻击 (Timeout)**: Provider超时或无法响应
- **混合攻击 (Mixed Attack)**: 组合多种攻击方式

**使用示例：**
```rust
use ya_verifier::{run_all_attack_tests, generate_test_report};

// 运行所有攻击测试
let results = run_all_attack_tests().await;

// 生成测试报告
let report = generate_test_report(&results);
println!("{}", report);
```

更多详细信息请参考 `core/verifier/README.md` 和 `core/verifier/IMPLEMENTATION_SUMMARY.md`。

## 验证流程

1. **任务分发**: Requestor将同一任务分发给多个Provider（至少3个）
2. **结果收集**: ResultCollector并发收集所有Provider的执行结果
3. **结果验证**: Verifier使用BFT算法验证结果
   - 如果≥2/3的Provider返回一致结果 → 达成共识，接受该结果
   - 如果无法达成共识 → 拒绝所有结果，可能需要重新执行
4. **惩罚机制**: 识别出的恶意Provider会被标记，触发Slashing惩罚

## 配置参数

### SlashingConfig (ya-verifier)
- `penalty_mode`: 惩罚模式（FixedAmount, Proportional, Hybrid）
- `base_penalty`: 基础惩罚金额
- `base_slash_ratio`: 基础惩罚比例
- `severity_ratios`: 不同严重程度的惩罚比例
- 更多配置请参考 `ya-verifier::SlashingConfig`

### CollectorConfig
- `collection_timeout`: 总收集超时（默认：300秒）
- `provider_timeout`: 单个Provider请求超时（默认：60秒）

## 攻击测试

运行攻击测试以验证系统的健壮性：

```bash
# 运行 ya-verifier 的攻击测试
cargo test --package ya-verifier --lib attack_tests

# 或者运行所有测试
cargo test --package ya-verifier
```

测试场景包括：
1. 少数节点共谋（1/5恶意节点）
2. 多数节点共谋（3/5恶意节点）
3. 随机错误攻击
4. 超时攻击
5. 混合攻击
6. 严重程度评估测试
7. 惩罚机制测试

## 集成到Requestor

要集成验证器到现有的Requestor流程中，需要：

1. 修改任务分发逻辑，支持向多个Provider分发同一任务
2. 使用ResultCollector收集结果
3. 根据VerificationResult决定是否接受结果
4. 对恶意Provider触发惩罚机制

## 性能考虑

- 验证器使用SHA-256哈希进行结果比较
- 结果收集是并发的，不会显著增加总执行时间
- 建议至少使用3个Provider以确保安全性
- 验证器服务基于Actix Actor模型，支持异步并发处理

## 安全保证

- **Byzantine Fault Tolerance**: 可以容忍最多1/3的恶意节点
- **共识机制**: 需要≥2/3的节点同意才能接受结果
- **惩罚机制**: 恶意节点会被识别并受到惩罚

## 模块整合说明

本模块（`ya-activity::requestor`）已整合使用独立的 `ya-verifier` 模块：

- ✅ 验证器功能已迁移到 `core/verifier/`
- ✅ `ResultCollector` 使用 `ya-verifier` 的共识引擎
- ✅ 惩罚机制由 `ya-verifier` 提供
- ✅ 攻击测试由 `ya-verifier` 提供

## 相关文档

- **验证器主文档**: `core/verifier/README.md`
- **实现总结**: `core/verifier/IMPLEMENTATION_SUMMARY.md`
- **严重程度评估**: `core/verifier/SEVERITY_EVALUATION.md`

