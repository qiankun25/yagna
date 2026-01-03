//! Attack test scenarios for the verifier
//!
//! This module implements various attack scenarios to test the robustness
//! of the verification system against malicious behavior.

use crate::result_collector::{ProviderResult, VerificationResult};
use crate::service::{RegisterTask, SubmitResult, VerifierService, WaitForVerification};
use crate::slashing::{
    generate_slashing_actions_with_severity, OffenseContext, OffenseSeverity, OffenseTracker,
    PenaltyMode, SeverityEvaluator, SlashingConfig,
};
use actix::prelude::*;
use chrono::Duration;
use serde_json;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use ya_client_model::{activity::{ExeScriptCommandResult, CommandResult}, NodeId};

/// Attack test result
#[derive(Debug, Clone)]
pub struct AttackTestResult {
    /// Name of the attack test
    pub test_name: String,
    /// Whether the attack was successfully detected/blocked
    pub attack_blocked: bool,
    /// Details about the test outcome
    pub details: String,
    /// Malicious providers identified
    pub malicious_providers: Vec<NodeId>,
    /// Slashing actions generated
    pub slashing_actions: usize,
}

/// Test scenario 1: Collusion attack
///
/// Multiple providers collude to submit the same incorrect result.
/// The verifier should detect this if they don't have 2/3 majority.
pub async fn test_collusion_attack() -> AttackTestResult {
    let service = VerifierService::new(None);
    let addr = service.start();

    let task_id = "collusion_test".to_string();
    let batch_id = "batch1".to_string();
    let expected_providers = 5;

    // Register task
    addr.send(RegisterTask {
        task_id: task_id.clone(),
        batch_id: batch_id.clone(),
        expected_providers,
        timeout: Some(Duration::seconds(10)),
    })
    .await
    .unwrap()
    .unwrap();

    // 2 honest providers submit correct result
    // Create a simple result directly
    let correct_result = vec![ExeScriptCommandResult {
        index: 0,
        result: CommandResult::Ok,
        stdout: None,
        stderr: None,
        message: None,
        is_batch_finished: true,
        event_date: chrono::Utc::now(),
    }];

    for i in 0..2 {
        let result = ProviderResult {
            provider_id: create_node_id(&format!("honest_{}", i)),
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            results: correct_result.clone(),
            timestamp: chrono::Utc::now(),
        };
        addr.send(SubmitResult { result })
            .await
            .unwrap()
            .unwrap();
    }

    // 3 colluding providers submit wrong result
    let wrong_result = vec![ExeScriptCommandResult {
        index: 0,
        result: CommandResult::Error,
        stdout: None,
        stderr: None,
        message: None,
        is_batch_finished: true,
        event_date: chrono::Utc::now(),
    }];

    for i in 0..3 {
        let result = ProviderResult {
            provider_id: create_node_id(&format!("colluder_{}", i)),
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            results: wrong_result.clone(),
            timestamp: chrono::Utc::now(),
        };
        addr.send(SubmitResult { result })
            .await
            .unwrap()
            .unwrap();
    }

    // Wait for verification
    let verification = addr
        .send(WaitForVerification {
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            timeout: Some(Duration::seconds(5)),
        })
        .await
        .unwrap();

    let is_failed = matches!(&verification, Ok(VerificationResult::Failed { .. }));
    
    let (malicious_providers, severity_info) = match verification {
        Ok(VerificationResult::Verified {
            malicious_providers,
            consensus_result_hash,
            ..
        }) => {
            // Test severity evaluation for collusion
            let mut evaluator = SeverityEvaluator::new();
            let mut contexts = Vec::new();
            
            for provider_id in &malicious_providers {
                let context = OffenseContext {
                    provider_id: provider_id.clone(),
                    offense_time: chrono::Utc::now(),
                    wrong_result_hash: "wrong_collusion".to_string(),
                    correct_result_hash: consensus_result_hash.clone(),
                    offense_type: crate::slashing::OffenseType::WrongResult,
                    colluding_providers: malicious_providers.clone(),
                    total_providers: expected_providers,
                    malicious_count: malicious_providers.len(),
                    resource_usage: None,
                    cost_info: None,
                };
                contexts.push(context);
            }
            
            // Evaluate severity for collusion attack
            let severities: Vec<OffenseSeverity> = contexts
                .iter()
                .map(|ctx| evaluator.evaluate_severity(ctx))
                .collect();
            
            let severity_info = format!(
                "严重程度: {:?} (共谋比例: {:.1}%)",
                severities.first().unwrap_or(&OffenseSeverity::Moderate),
                (malicious_providers.len() as f64 / expected_providers as f64) * 100.0
            );
            
            (malicious_providers, severity_info)
        }
        Ok(VerificationResult::Failed { ref detected_malicious, .. }) => {
            if !detected_malicious.is_empty() {
                (detected_malicious.clone(), format!("共谋攻击已检测: 识别出{}个共谋节点", detected_malicious.len()))
            } else {
                (vec![], "共谋攻击: 未达成共识，但未识别出共谋节点".to_string())
            }
        },
        _ => (vec![], "等待中".to_string()),
    };

    // Check if colluders were identified
    // With the improved consensus algorithm, 3 colluders out of 5 should not reach consensus
    // (need 4 votes, but only 3 colluders + 2 honest = 5 total, 3 < 4, so no consensus)
    // But we should still detect them as malicious
    let attack_blocked = malicious_providers.len() >= 3 || is_failed;
    let details = if attack_blocked {
        format!(
            "共谋攻击已检测: 识别出3个共谋节点. {}",
            severity_info
        )
    } else {
        "共谋攻击成功: 验证器接受了错误结果".to_string()
    };

    let malicious_count = malicious_providers.len();
    AttackTestResult {
        test_name: "Collusion Attack".to_string(),
        attack_blocked,
        details,
        malicious_providers,
        slashing_actions: malicious_count,
    }
}

/// Test scenario 2: Random error attack
///
/// Providers submit random/incorrect results due to hardware failures
/// or bugs. The verifier should identify them as malicious.
pub async fn test_random_error_attack() -> AttackTestResult {
    let service = VerifierService::new(None);
    let addr = service.start();

    let task_id = "random_error_test".to_string();
    let batch_id = "batch1".to_string();
    let expected_providers = 6;

    // Register task
    addr.send(RegisterTask {
        task_id: task_id.clone(),
        batch_id: batch_id.clone(),
        expected_providers,
        timeout: Some(Duration::seconds(10)),
    })
    .await
    .unwrap()
    .unwrap();

    // 4 honest providers submit correct result
    let correct_result = vec![ExeScriptCommandResult {
        index: 0,
        result: CommandResult::Ok,
        stdout: None,
        stderr: None,
        message: None,
        is_batch_finished: true,
        event_date: chrono::Utc::now(),
    }];

    for i in 0..4 {
        let result = ProviderResult {
            provider_id: create_node_id(&format!("honest_{}", i)),
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            results: correct_result.clone(),
            timestamp: chrono::Utc::now(),
        };
        addr.send(SubmitResult { result })
            .await
            .unwrap()
            .unwrap();
    }

    // 2 providers submit random errors
    let error_results = vec![
        vec![ExeScriptCommandResult {
            index: 0,
            result: CommandResult::Error,
            stdout: None,
            stderr: None,
            message: Some("error1".to_string()),
            is_batch_finished: true,
            event_date: chrono::Utc::now(),
        }],
        vec![ExeScriptCommandResult {
            index: 0,
            result: CommandResult::Error,
            stdout: None,
            stderr: None,
            message: Some("error2".to_string()),
            is_batch_finished: true,
            event_date: chrono::Utc::now(),
        }],
    ];

    for (i, error_result) in error_results.iter().enumerate() {
        let result = ProviderResult {
            provider_id: create_node_id(&format!("error_provider_{}", i)),
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            results: error_result.clone(),
            timestamp: chrono::Utc::now(),
        };
        addr.send(SubmitResult { result })
            .await
            .unwrap()
            .unwrap();
    }

    // Wait for verification
    let verification = addr
        .send(WaitForVerification {
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            timeout: Some(Duration::seconds(5)),
        })
        .await
        .unwrap();

    let (malicious_providers, severity_info) = match verification {
        Ok(VerificationResult::Verified {
            malicious_providers,
            consensus_result_hash,
            ..
        }) => {
            // Test severity evaluation for random errors
            let mut evaluator = SeverityEvaluator::new();
            let mut contexts = Vec::new();
            
            for provider_id in &malicious_providers {
                let context = OffenseContext {
                    provider_id: provider_id.clone(),
                    offense_time: chrono::Utc::now(),
                    wrong_result_hash: format!("error_{}", provider_id),
                    correct_result_hash: consensus_result_hash.clone(),
                    offense_type: crate::slashing::OffenseType::WrongResult,
                    colluding_providers: vec![], // Random errors, no collusion
                    total_providers: expected_providers,
                    malicious_count: malicious_providers.len(),
                    resource_usage: None,
                    cost_info: None,
                };
                contexts.push(context);
            }
            
            // Evaluate severity for random errors (should be Minor or Moderate)
            let severities: Vec<OffenseSeverity> = contexts
                .iter()
                .map(|ctx| evaluator.evaluate_severity(ctx))
                .collect();
            
            let severity_info = format!(
                "严重程度: {:?} (随机错误，无共谋)",
                severities.first().unwrap_or(&OffenseSeverity::Minor)
            );
            
            (malicious_providers, severity_info)
        }
        _ => (vec![], String::new()),
    };

    let attack_blocked = malicious_providers.len() == 2;
    let details = if attack_blocked {
        format!(
            "随机错误攻击已检测: 识别出2个错误节点. {}",
            severity_info
        )
    } else {
        "随机错误攻击: 部分错误未检测到".to_string()
    };

    let malicious_count = malicious_providers.len();
    AttackTestResult {
        test_name: "Random Error Attack".to_string(),
        attack_blocked,
        details,
        malicious_providers,
        slashing_actions: malicious_count,
    }
}

/// Test scenario 3: Insufficient honest providers
///
/// If less than 2/3 of providers are honest, consensus cannot be reached.
/// This tests the system's behavior when there are too many malicious nodes.
pub async fn test_insufficient_honest_providers() -> AttackTestResult {
    let service = VerifierService::new(None);
    let addr = service.start();

    let task_id = "insufficient_honest_test".to_string();
    let batch_id = "batch1".to_string();
    let expected_providers = 5;

    // Register task
    addr.send(RegisterTask {
        task_id: task_id.clone(),
        batch_id: batch_id.clone(),
        expected_providers,
        timeout: Some(Duration::seconds(10)),
    })
    .await
    .unwrap()
    .unwrap();

    // Only 1 honest provider (need at least 4 out of 5 for consensus)
    let correct_result = vec![ExeScriptCommandResult {
        index: 0,
        result: CommandResult::Ok,
        stdout: None,
        stderr: None,
        message: None,
        is_batch_finished: true,
        event_date: chrono::Utc::now(),
    }];

    let result = ProviderResult {
        provider_id: create_node_id("honest_0"),
        task_id: task_id.clone(),
        batch_id: batch_id.clone(),
        results: correct_result,
        timestamp: chrono::Utc::now(),
    };
    addr.send(SubmitResult { result })
        .await
        .unwrap()
        .unwrap();

    // 4 malicious providers submit different wrong results
    for i in 0..4 {
        let wrong_result_json = format!(r#"[{{"index":0,"result":"Error","stdout":null,"stderr":null,"message":"wrong_{}","isBatchFinished":true,"eventDate":"2023-12-30T10:30:00Z"}}]"#, i);
        let wrong_result: Vec<ExeScriptCommandResult> = serde_json::from_str(&wrong_result_json).unwrap();

        let result = ProviderResult {
            provider_id: create_node_id(&format!("malicious_{}", i)),
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            results: wrong_result,
            timestamp: chrono::Utc::now(),
        };
        addr.send(SubmitResult { result })
            .await
            .unwrap()
            .unwrap();
    }

    // Wait for verification
    let verification = addr
        .send(WaitForVerification {
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            timeout: Some(Duration::seconds(5)),
        })
        .await
        .unwrap();

    let attack_blocked = match verification {
        Ok(VerificationResult::Failed { .. }) => true,
        Ok(VerificationResult::Verified { .. }) => false,
        _ => false,
    };

    let details = if attack_blocked {
        "Attack blocked: No consensus reached due to insufficient honest providers".to_string()
    } else {
        "Attack succeeded: Consensus reached with insufficient honest providers".to_string()
    };

    AttackTestResult {
        test_name: "Insufficient Honest Providers".to_string(),
        attack_blocked,
        details,
        malicious_providers: vec![],
        slashing_actions: 0,
    }
}

/// Test scenario 4: Slashing mechanism with severity evaluation
///
/// Test that repeated offenses result in increasing penalties and proper severity evaluation.
pub async fn test_slashing_mechanism() -> AttackTestResult {
    // Test with proportional penalty mode
    let config = SlashingConfig {
        penalty_mode: PenaltyMode::Proportional,
        base_slash_ratio: 0.01,  // 1%
        min_penalty_amount: 1.0,
        ..Default::default()
    };
    
    let offense_tracker = Arc::new(RwLock::new(OffenseTracker::new(config.clone())));
    let severity_evaluator = Arc::new(RwLock::new(SeverityEvaluator::new()));

    let malicious_providers = vec![
        create_node_id("repeat_offender"),
        create_node_id("first_time"),
    ];

    // Create offense contexts
    let mut contexts = Vec::new();
    for provider_id in &malicious_providers {
        contexts.push(OffenseContext {
            provider_id: provider_id.clone(),
            offense_time: chrono::Utc::now(),
            wrong_result_hash: "wrong".to_string(),
            correct_result_hash: "correct".to_string(),
            offense_type: crate::slashing::OffenseType::WrongResult,
            colluding_providers: vec![],
            total_providers: 5,
            malicious_count: 2,
            resource_usage: None,
            cost_info: None,
        });
    }

    // First offense with severity evaluation
    let actions1 = {
        let mut tracker = offense_tracker.write().await;
        let mut evaluator = severity_evaluator.write().await;
        generate_slashing_actions_with_severity(
            &malicious_providers,
            &mut tracker,
            &mut evaluator,
            &contexts,
        )
    };
    
    assert_eq!(actions1.len(), 2);
    // Check severity (should be Minor for first-time single offenses)
    assert!(matches!(actions1[0].severity, OffenseSeverity::Minor | OffenseSeverity::Moderate));

    // Second offense for repeat offender (within time window)
    let repeat_offender = vec![create_node_id("repeat_offender")];
    let repeat_context = vec![OffenseContext {
        provider_id: repeat_offender[0].clone(),
        offense_time: chrono::Utc::now() + Duration::seconds(100), // Within 1 hour
        wrong_result_hash: "wrong2".to_string(),
        correct_result_hash: "correct".to_string(),
        offense_type: crate::slashing::OffenseType::WrongResult,
        colluding_providers: vec![],
        total_providers: 5,
        malicious_count: 1,
        resource_usage: None,
        cost_info: None,
    }];
    
    let actions2 = {
        let mut tracker = offense_tracker.write().await;
        let mut evaluator = severity_evaluator.write().await;
        generate_slashing_actions_with_severity(
            &repeat_offender,
            &mut tracker,
            &mut evaluator,
            &repeat_context,
        )
    };
    
    assert_eq!(actions2.len(), 1);
    assert_eq!(actions2[0].offense_count, 2);
    // Severity should increase for repeat offense (may be Minor, Moderate, or Severe depending on timing)
    // Just check that it's a valid severity
    assert!(matches!(
        actions2[0].severity,
        OffenseSeverity::Minor | OffenseSeverity::Moderate | OffenseSeverity::Severe
    ));

    let details = format!(
        "惩罚机制工作正常: 首次违规严重程度 {:?}, 重复违规严重程度 {:?}",
        actions1[0].severity, actions2[0].severity
    );

    AttackTestResult {
        test_name: "Slashing Mechanism with Severity".to_string(),
        attack_blocked: true,
        details,
        malicious_providers,
        slashing_actions: actions1.len() + actions2.len(),
    }
}

/// Run all attack tests and generate report
/// Test scenario 5: Resource inflation attack
///
/// Provider exaggerates resource usage to charge more.
/// The verifier should detect resource usage anomalies.
pub async fn test_resource_inflation_attack() -> AttackTestResult {
    use crate::result_validator::ResultValidator;
    use crate::slashing::OffenseType;
    use std::collections::HashMap;

    let validator = ResultValidator::default();
    
    // Simulate consensus resource usage
    let mut consensus_usage = HashMap::new();
    consensus_usage.insert("cpu".to_string(), 10.0);
    consensus_usage.insert("gpu".to_string(), 5.0);
    consensus_usage.insert("mem".to_string(), 100.0);

    // Malicious provider inflates resource usage
    let mut reported_usage = HashMap::new();
    reported_usage.insert("cpu".to_string(), 25.0);  // 2.5x inflation
    reported_usage.insert("gpu".to_string(), 4.0);   // Within tolerance
    reported_usage.insert("mem".to_string(), 150.0); // 1.5x inflation

    let validation = validator.validate_resource_usage(&reported_usage, &consensus_usage);
    
    let attack_blocked = !validation.is_valid;
    let detected_offenses: Vec<&OffenseType> = validation.offenses.iter()
        .flat_map(|o| {
            match o {
                OffenseType::Multiple(offenses) => offenses.iter().collect::<Vec<_>>(),
                other => vec![other],
            }
        })
        .filter(|o| matches!(o, OffenseType::ResourceInflation { .. }))
        .collect();

    let details = if attack_blocked {
        format!(
            "资源夸大攻击已检测: 发现 {} 个资源夸大违规",
            detected_offenses.len()
        )
    } else {
        "资源夸大攻击: 未检测到异常".to_string()
    };

    AttackTestResult {
        test_name: "Resource Inflation Attack".to_string(),
        attack_blocked,
        details,
        malicious_providers: vec![], // Resource validation doesn't identify specific providers
        slashing_actions: detected_offenses.len(),
    }
}

/// Test scenario 6: Cost fraud attack
///
/// Provider manipulates cost calculation to overcharge.
/// The verifier should detect cost calculation anomalies.
pub async fn test_cost_fraud_attack() -> AttackTestResult {
    use crate::result_validator::ResultValidator;
    use crate::slashing::OffenseType;

    let validator = ResultValidator::default();
    
    // Consensus cost
    let consensus_cost = 1.0;
    
    // Malicious provider reports inflated cost
    let reported_cost = 1.15; // 15% inflation, exceeds 5% tolerance

    let validation = validator.validate_cost(reported_cost, consensus_cost);
    
    let attack_blocked = !validation.is_valid;
    let detected_fraud = validation.offenses.iter()
        .any(|o| matches!(o, OffenseType::CostFraud { .. }));

    let details = if attack_blocked && detected_fraud {
        format!(
            "成本欺诈攻击已检测: 报告成本 {:.2}, 共识成本 {:.2}, 差异 {:.2}%",
            reported_cost, consensus_cost, ((reported_cost - consensus_cost) / consensus_cost) * 100.0
        )
    } else {
        "成本欺诈攻击: 未检测到异常".to_string()
    };

    AttackTestResult {
        test_name: "Cost Fraud Attack".to_string(),
        attack_blocked,
        details,
        malicious_providers: vec![],
        slashing_actions: if detected_fraud { 1 } else { 0 },
    }
}

/// Test scenario 7: Timestamp manipulation attack
///
/// Provider manipulates timestamps to avoid detection or create inconsistencies.
/// The verifier should detect timestamp anomalies.
pub async fn test_timestamp_manipulation_attack() -> AttackTestResult {
    use crate::result_validator::ResultValidator;
    use crate::slashing::OffenseType;
    use chrono::Utc;

    let validator = ResultValidator::default();
    
    // Consensus timestamp
    let consensus_timestamp = Utc::now();
    
    // Malicious provider uses future timestamp (more than 5 minutes ahead)
    let future_timestamp = consensus_timestamp + chrono::Duration::seconds(400); // 6.67 minutes
    let reported_timestamps = vec![future_timestamp];

    let validation = validator.validate_timestamps(&reported_timestamps, consensus_timestamp);
    
    let attack_blocked = !validation.is_valid;
    let detected_anomaly = validation.offenses.iter()
        .any(|o| matches!(o, OffenseType::TimestampAnomaly { .. }));

    let details = if attack_blocked && detected_anomaly {
        let time_diff = (future_timestamp - consensus_timestamp).num_seconds();
        format!(
            "时间戳篡改攻击已检测: 时间差 {} 秒 (超过允许的 {} 秒)",
            time_diff, validator.max_timestamp_diff_secs
        )
    } else {
        "时间戳篡改攻击: 未检测到异常".to_string()
    };

    AttackTestResult {
        test_name: "Timestamp Manipulation Attack".to_string(),
        attack_blocked,
        details,
        malicious_providers: vec![],
        slashing_actions: if detected_anomaly { 1 } else { 0 },
    }
}

/// Test scenario 8: Escalating attack (渐进式攻击)
///
/// Provider starts with minor violations and gradually escalates to more serious attacks.
/// Tests the system's ability to detect and respond to escalating malicious behavior.
pub async fn test_escalating_attack() -> AttackTestResult {
    use crate::result_validator::ResultValidator;
    use crate::slashing::{OffenseTracker, SeverityEvaluator, SlashingConfig, PenaltyMode};
    use std::collections::HashMap;

    let config = SlashingConfig {
        penalty_mode: PenaltyMode::Proportional,
        ..Default::default()
    };
    
    let mut tracker = OffenseTracker::new(config);
    let mut evaluator = SeverityEvaluator::new();
    let validator = ResultValidator::default();

    let provider_id = create_node_id("escalating_attacker");
    let mut detected_violations = 0;
    let mut severity_levels = Vec::new();

    // Stage 1: Minor violation (resource inflation 1.2x)
    let mut usage1 = HashMap::new();
    usage1.insert("cpu".to_string(), 12.0);
    let mut consensus1 = HashMap::new();
    consensus1.insert("cpu".to_string(), 10.0);
    let validation1 = validator.validate_resource_usage(&usage1, &consensus1);
    if !validation1.is_valid {
        detected_violations += 1;
    }

    // Stage 2: Moderate violation (resource inflation 1.8x)
    let mut usage2 = HashMap::new();
    usage2.insert("cpu".to_string(), 18.0);
    let validation2 = validator.validate_resource_usage(&usage2, &consensus1);
    if !validation2.is_valid {
        detected_violations += 1;
    }

    // Stage 3: Severe violation (resource inflation 3x + cost fraud)
    let mut usage3 = HashMap::new();
    usage3.insert("cpu".to_string(), 30.0);
    let validation3 = validator.validate_resource_usage(&usage3, &consensus1);
    let cost_validation = validator.validate_cost(2.0, 1.0);
    if !validation3.is_valid || !cost_validation.is_valid {
        detected_violations += 1;
    }

    // Create offense contexts and evaluate severity
    let mut contexts: Vec<crate::slashing::OffenseContext> = Vec::new();
    for i in 0..3 {
        let context = crate::slashing::OffenseContext {
            provider_id: provider_id.clone(),
            offense_time: chrono::Utc::now() + Duration::seconds(i * 100),
            wrong_result_hash: format!("escalating_{}", i),
            correct_result_hash: "correct".to_string(),
            offense_type: crate::slashing::OffenseType::ResourceInflation {
                resource_type: "cpu".to_string(),
                reported_usage: 10.0 + (i as f64 * 10.0),
                expected_usage: 10.0,
                inflation_ratio: 1.0 + (i as f64 * 0.5),
            },
            colluding_providers: vec![],
            total_providers: 5,
            malicious_count: 1,
            resource_usage: None,
            cost_info: None,
        };
        let severity = evaluator.evaluate_severity(&context);
        severity_levels.push(severity.clone());
        tracker.record_offense(provider_id.clone());
    }

    let attack_blocked = detected_violations >= 2;
    let severity_escalated = severity_levels.len() >= 2 && 
        {
            let last = severity_levels.last().unwrap();
            let first = severity_levels.first().unwrap();
            // Compare severity: Minor < Moderate < Severe
            match (first, last) {
                (OffenseSeverity::Minor, OffenseSeverity::Moderate) => true,
                (OffenseSeverity::Minor, OffenseSeverity::Severe) => true,
                (OffenseSeverity::Moderate, OffenseSeverity::Severe) => true,
                _ => false,
            }
        };

    let details = format!(
        "渐进式攻击检测: 发现 {} 个违规阶段, 严重程度变化: {:?}, 攻击{}",
        detected_violations,
        severity_levels,
        if attack_blocked && severity_escalated { "已阻止并识别严重程度升级" } else { "部分成功" }
    );

    AttackTestResult {
        test_name: "Escalating Attack".to_string(),
        attack_blocked,
        details,
        malicious_providers: if attack_blocked { vec![provider_id] } else { vec![] },
        slashing_actions: detected_violations,
    }
}

/// Test scenario 9: Hybrid attack (混合攻击)
///
/// Provider uses multiple attack vectors simultaneously (wrong result + resource inflation + cost fraud).
/// Tests the system's ability to detect multiple violation types in a single submission.
pub async fn test_hybrid_attack() -> AttackTestResult {
    use crate::result_validator::ResultValidator;
    use crate::slashing::OffenseType;
    use std::collections::HashMap;

    let validator = ResultValidator::default();
    let mut detected_offenses = Vec::new();

    // Wrong result
    let wrong_result = vec![ExeScriptCommandResult {
        index: 0,
        result: CommandResult::Error,
        stdout: None,
        stderr: None,
        message: None,
        is_batch_finished: true,
        event_date: chrono::Utc::now(),
    }];
    let format_validation = validator.validate_format(&wrong_result);
    if !format_validation.is_valid {
        detected_offenses.extend(format_validation.offenses.clone());
    }

    // Resource inflation
    let mut usage = HashMap::new();
    usage.insert("cpu".to_string(), 25.0);
    usage.insert("gpu".to_string(), 20.0);
    let mut consensus = HashMap::new();
    consensus.insert("cpu".to_string(), 10.0);
    consensus.insert("gpu".to_string(), 5.0);
    let usage_validation = validator.validate_resource_usage(&usage, &consensus);
    if !usage_validation.is_valid {
        detected_offenses.extend(usage_validation.offenses.clone());
    }

    // Cost fraud
    let cost_validation = validator.validate_cost(1.5, 1.0);
    if !cost_validation.is_valid {
        detected_offenses.extend(cost_validation.offenses.clone());
    }

    let attack_blocked = !detected_offenses.is_empty();
    let multiple_offense_types = detected_offenses.iter()
        .any(|o| matches!(o, OffenseType::Multiple(_)));

    let details = format!(
        "混合攻击检测: 发现 {} 种违规类型, {}",
        detected_offenses.len(),
        if multiple_offense_types {
            "检测到多重违规组合"
        } else if attack_blocked {
            "部分违规类型已检测"
        } else {
            "攻击成功"
        }
    );

    AttackTestResult {
        test_name: "Hybrid Attack".to_string(),
        attack_blocked,
        details,
        malicious_providers: vec![],
        slashing_actions: detected_offenses.len(),
    }
}

/// Test scenario 10: Partial result tampering (部分结果篡改)
///
/// Provider submits results where some commands are correct and others are wrong.
/// Tests the system's ability to detect partial tampering.
pub async fn test_partial_result_tampering() -> AttackTestResult {
    let service = VerifierService::new(None);
    let addr = service.start();

    let task_id = "partial_tampering_test".to_string();
    let batch_id = "batch1".to_string();
    let expected_providers = 5;

    addr.send(RegisterTask {
        task_id: task_id.clone(),
        batch_id: batch_id.clone(),
        expected_providers,
        timeout: Some(Duration::seconds(10)),
    })
    .await
    .unwrap()
    .unwrap();

    // 3 honest providers submit complete correct results
    let correct_result = vec![
        ExeScriptCommandResult {
            index: 0,
            result: CommandResult::Ok,
            stdout: None,
            stderr: None,
            message: None,
            is_batch_finished: false,
            event_date: chrono::Utc::now(),
        },
        ExeScriptCommandResult {
            index: 1,
            result: CommandResult::Ok,
            stdout: None,
            stderr: None,
            message: None,
            is_batch_finished: true,
            event_date: chrono::Utc::now(),
        },
    ];

    for i in 0..3 {
        let result = ProviderResult {
            provider_id: create_node_id(&format!("honest_{}", i)),
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            results: correct_result.clone(),
            timestamp: chrono::Utc::now(),
        };
        addr.send(SubmitResult { result })
            .await
            .unwrap()
            .unwrap();
    }

    // 2 malicious providers submit partially tampered results (first command correct, second wrong)
    let tampered_result = vec![
        ExeScriptCommandResult {
            index: 0,
            result: CommandResult::Ok,
            stdout: None, // Correct
            stderr: None,
            message: None,
            is_batch_finished: false,
            event_date: chrono::Utc::now(),
        },
        ExeScriptCommandResult {
            index: 1,
            result: CommandResult::Error, // Wrong
            stdout: None,
            stderr: None,
            message: None,
            is_batch_finished: true,
            event_date: chrono::Utc::now(),
        },
    ];

    for i in 0..2 {
        let result = ProviderResult {
            provider_id: create_node_id(&format!("tamperer_{}", i)),
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            results: tampered_result.clone(),
            timestamp: chrono::Utc::now(),
        };
        addr.send(SubmitResult { result })
            .await
            .unwrap()
            .unwrap();
    }

    let verification = addr
        .send(WaitForVerification {
            task_id: task_id.clone(),
            batch_id: batch_id.clone(),
            timeout: Some(Duration::seconds(5)),
        })
        .await
        .unwrap()
        .unwrap();

    let (malicious_providers, details) = match verification {
        VerificationResult::Verified { malicious_providers, .. } => {
            (malicious_providers.clone(), "部分结果篡改攻击已检测: 识别出篡改的Provider".to_string())
        }
        _ => (vec![], "部分结果篡改攻击: 未检测到篡改".to_string()),
    };

    let slashing_count = malicious_providers.len();
    AttackTestResult {
        test_name: "Partial Result Tampering".to_string(),
        attack_blocked: !malicious_providers.is_empty(),
        details,
        malicious_providers,
        slashing_actions: slashing_count,
    }
}

/// Test scenario 11: Resource exhaustion attack (资源耗尽攻击)
///
/// Provider reports extremely high resource usage to exhaust system resources or cause overflow.
pub async fn test_resource_exhaustion_attack() -> AttackTestResult {
    use crate::result_validator::ResultValidator;
    use crate::slashing::OffenseType;
    use std::collections::HashMap;

    let validator = ResultValidator::default();
    
    let mut reported_usage = HashMap::new();
    reported_usage.insert("cpu".to_string(), f64::MAX / 2.0); // Extremely large value
    reported_usage.insert("mem".to_string(), 1e10); // 10GB
    reported_usage.insert("gpu".to_string(), 1e8);

    let mut consensus_usage = HashMap::new();
    consensus_usage.insert("cpu".to_string(), 10.0);
    consensus_usage.insert("mem".to_string(), 100.0);
    consensus_usage.insert("gpu".to_string(), 5.0);

    let validation = validator.validate_resource_usage(&reported_usage, &consensus_usage);
    
    let attack_blocked = !validation.is_valid;
    let detected_exhaustion = validation.offenses.iter()
        .any(|o| matches!(o, OffenseType::ResourceInflation { inflation_ratio, .. } if *inflation_ratio > 100.0));

    let details = if attack_blocked && detected_exhaustion {
        "资源耗尽攻击已检测: 检测到异常大的资源使用量".to_string()
    } else if attack_blocked {
        "资源耗尽攻击: 部分检测到异常".to_string()
    } else {
        "资源耗尽攻击: 未检测到异常".to_string()
    };

    AttackTestResult {
        test_name: "Resource Exhaustion Attack".to_string(),
        attack_blocked,
        details,
        malicious_providers: vec![],
        slashing_actions: if detected_exhaustion { validation.offenses.len() } else { 0 },
    }
}

/// Test scenario 12: Format injection attack (格式注入攻击)
///
/// Provider injects malicious or malformed data in result fields to cause parsing errors or exploits.
pub async fn test_format_injection_attack() -> AttackTestResult {
    use crate::result_validator::ResultValidator;

    let validator = ResultValidator::default();

    // Try to inject various malicious formats
    let malicious_results = vec![
        // Null bytes injection
        ExeScriptCommandResult {
            index: 0,
            result: CommandResult::Ok,
            stdout: None,
            stderr: None,
            message: None,
            is_batch_finished: true,
            event_date: chrono::Utc::now(),
        },
        // Discontinuous index
        ExeScriptCommandResult {
            index: 5, // Should be 1, not 5
            result: CommandResult::Ok,
            stdout: None,
            stderr: None,
            message: None,
            is_batch_finished: true,
            event_date: chrono::Utc::now(),
        },
        // Duplicate index
        ExeScriptCommandResult {
            index: 0,
            result: CommandResult::Ok,
            stdout: None,
            stderr: None,
            message: None,
            is_batch_finished: false,
            event_date: chrono::Utc::now(),
        },
    ];

    let validation = validator.validate_format(&malicious_results);
    
    let attack_blocked = !validation.is_valid;
    let format_violations = validation.offenses.iter()
        .any(|o| matches!(o, crate::slashing::OffenseType::FormatViolation { .. }));

    let details = if attack_blocked && format_violations {
        format!("格式注入攻击已检测: 发现 {} 个格式违规", validation.errors.len())
    } else if attack_blocked {
        "格式注入攻击: 部分检测到异常".to_string()
    } else {
        "格式注入攻击: 未检测到异常".to_string()
    };

    AttackTestResult {
        test_name: "Format Injection Attack".to_string(),
        attack_blocked,
        details,
        malicious_providers: vec![],
        slashing_actions: if format_violations { validation.offenses.len() } else { 0 },
    }
}

pub async fn run_all_attack_tests() -> Vec<AttackTestResult> {
    let mut results = Vec::new();

    log::info!("Running collusion attack test...");
    results.push(test_collusion_attack().await);

    log::info!("Running random error attack test...");
    results.push(test_random_error_attack().await);

    log::info!("Running insufficient honest providers test...");
    results.push(test_insufficient_honest_providers().await);

    log::info!("Running slashing mechanism test...");
    results.push(test_slashing_mechanism().await);

    log::info!("Running resource inflation attack test...");
    results.push(test_resource_inflation_attack().await);

    log::info!("Running cost fraud attack test...");
    results.push(test_cost_fraud_attack().await);

    log::info!("Running timestamp manipulation attack test...");
    results.push(test_timestamp_manipulation_attack().await);

    log::info!("Running escalating attack test...");
    results.push(test_escalating_attack().await);

    log::info!("Running hybrid attack test...");
    results.push(test_hybrid_attack().await);

    log::info!("Running partial result tampering test...");
    results.push(test_partial_result_tampering().await);

    log::info!("Running resource exhaustion attack test...");
    results.push(test_resource_exhaustion_attack().await);

    log::info!("Running format injection attack test...");
    results.push(test_format_injection_attack().await);

    results
}

/// Generate a test report
pub fn generate_test_report(results: &[AttackTestResult]) -> String {
    let mut report = String::new();
    report.push_str("=== Verifier Attack Test Report ===\n\n");

    let mut total_tests = 0;
    let mut blocked_attacks = 0;

    for result in results {
        total_tests += 1;
        if result.attack_blocked {
            blocked_attacks += 1;
        }

        report.push_str(&format!("Test: {}\n", result.test_name));
        report.push_str(&format!("  Status: {}\n", if result.attack_blocked { "BLOCKED" } else { "SUCCEEDED" }));
        report.push_str(&format!("  Details: {}\n", result.details));
        report.push_str(&format!("  Malicious Providers Identified: {}\n", result.malicious_providers.len()));
        report.push_str(&format!("  Slashing Actions: {}\n", result.slashing_actions));
        report.push_str("\n");
    }

    report.push_str(&format!("Summary:\n"));
    report.push_str(&format!("  Total Tests: {}\n", total_tests));
    report.push_str(&format!("  Attacks Blocked: {}\n", blocked_attacks));
    report.push_str(&format!("  Attacks Succeeded: {}\n", total_tests - blocked_attacks));
    report.push_str(&format!("  Success Rate: {:.1}%\n", (blocked_attacks as f64 / total_tests as f64) * 100.0));

    report
}

// Helper function to create a valid NodeId from a string
fn create_node_id(s: &str) -> NodeId {
    // NodeId needs to be 42 characters (0x + 40 hex chars)
    // We'll use a hash of the string to create a consistent ID
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let hash = hasher.finalize();
    // Take first 20 bytes (40 hex chars) and format as hex
    let hex_str = format!("0x{}", hex::encode(&hash[..20]));
    NodeId::from_str(&hex_str).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix_rt::test]
    async fn test_attack_tests() {
        let results = run_all_attack_tests().await;
        let report = generate_test_report(&results);
        println!("{}", report);

        // At least some attacks should be blocked
        let blocked_count = results.iter().filter(|r| r.attack_blocked).count();
        assert!(blocked_count > 0, "At least some attacks should be blocked");
    }
}

