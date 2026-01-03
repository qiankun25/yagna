//! Verifier service implementation

use crate::error::{VerifierError, VerifierResult};
use crate::result_collector::{ProviderResult, ResultCollector, VerificationResult};
use crate::slashing::{
    generate_slashing_actions, generate_slashing_actions_advanced, generate_slashing_actions_with_severity,
    OffenseContext, OffenseTracker, SeverityEvaluator, SlashingAction, SlashingConfig,
};
use actix::prelude::*;
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use ya_client_model::NodeId;

/// Message to submit a result from a provider
#[derive(Message, Debug, Clone)]
#[rtype(result = "VerifierResult<()>")]
pub struct SubmitResult {
    pub result: ProviderResult,
}

/// Message to get verification status
#[derive(Message, Debug, Clone)]
#[rtype(result = "VerifierResult<VerificationResult>")]
pub struct GetVerificationStatus {
    pub task_id: String,
    pub batch_id: String,
}

/// Message to wait for verification with timeout
#[derive(Message, Debug, Clone)]
#[rtype(result = "VerifierResult<VerificationResult>")]
pub struct WaitForVerification {
    pub task_id: String,
    pub batch_id: String,
    pub timeout: Option<chrono::Duration>,
}

/// Message to register a task for verification
#[derive(Message, Debug, Clone)]
#[rtype(result = "VerifierResult<()>")]
pub struct RegisterTask {
    pub task_id: String,
    pub batch_id: String,
    pub expected_providers: usize,
    pub timeout: Option<chrono::Duration>,
}

/// Verifier service actor
pub struct VerifierService {
    /// Active task collectors
    collectors: Arc<RwLock<HashMap<String, Arc<RwLock<ResultCollector>>>>>,
    /// Offense tracker for slashing
    offense_tracker: Arc<RwLock<OffenseTracker>>,
    /// Severity evaluator
    severity_evaluator: Arc<RwLock<SeverityEvaluator>>,
    /// Slashing configuration
    slashing_config: SlashingConfig,
}

impl VerifierService {
    pub fn new(slashing_config: Option<SlashingConfig>) -> Self {
        Self {
            collectors: Arc::new(RwLock::new(HashMap::new())),
            offense_tracker: Arc::new(RwLock::new(OffenseTracker::new(
                slashing_config.clone().unwrap_or_default(),
            ))),
            severity_evaluator: Arc::new(RwLock::new(SeverityEvaluator::new())),
            slashing_config: slashing_config.unwrap_or_default(),
        }
    }

    fn get_collector_key(task_id: &str, batch_id: &str) -> String {
        format!("{}:{}", task_id, batch_id)
    }

    async fn get_or_create_collector(
        &self,
        task_id: String,
        batch_id: String,
        expected_providers: usize,
        timeout: Option<chrono::Duration>,
    ) -> VerifierResult<Arc<RwLock<ResultCollector>>> {
        let key = Self::get_collector_key(&task_id, &batch_id);
        let mut collectors = self.collectors.write().await;

        if let Some(collector) = collectors.get(&key) {
            return Ok(collector.clone());
        }

        let collector = Arc::new(RwLock::new(ResultCollector::new(
            task_id,
            batch_id,
            expected_providers,
            timeout,
        )));

        collectors.insert(key, collector.clone());
        Ok(collector)
    }

    async fn get_collector(
        &self,
        task_id: &str,
        batch_id: &str,
    ) -> VerifierResult<Arc<RwLock<ResultCollector>>> {
        let key = Self::get_collector_key(task_id, batch_id);
        let collectors = self.collectors.read().await;

        collectors
            .get(&key)
            .cloned()
            .ok_or_else(|| VerifierError::TaskNotFound(key))
    }
}

impl Actor for VerifierService {
    type Context = Context<Self>;
}

impl Handler<RegisterTask> for VerifierService {
    type Result = ResponseActFuture<Self, VerifierResult<()>>;

    fn handle(&mut self, msg: RegisterTask, _ctx: &mut Self::Context) -> Self::Result {
        let collectors = self.collectors.clone();

        let fut = async move {
            let key = VerifierService::get_collector_key(&msg.task_id, &msg.batch_id);
            let mut collectors = collectors.write().await;

            if collectors.contains_key(&key) {
                return Err(VerifierError::Internal(format!(
                    "Task {} already registered",
                    key
                )));
            }

            let collector = Arc::new(RwLock::new(ResultCollector::new(
                msg.task_id,
                msg.batch_id,
                msg.expected_providers,
                msg.timeout,
            )));

            collectors.insert(key, collector);
            Ok(())
        };

        Box::pin(actix::fut::wrap_future(fut))
    }
}

impl Handler<SubmitResult> for VerifierService {
    type Result = ResponseActFuture<Self, VerifierResult<()>>;

    fn handle(&mut self, msg: SubmitResult, _ctx: &mut Self::Context) -> Self::Result {
        let collectors = self.collectors.clone();
        let task_id = msg.result.task_id.clone();
        let batch_id = msg.result.batch_id.clone();

        let fut = async move {
            let collector = {
                let collectors = collectors.read().await;
                let key = VerifierService::get_collector_key(&task_id, &batch_id);
                collectors.get(&key).cloned()
            };

            let collector = match collector {
                Some(c) => c,
                None => {
                    return Err(VerifierError::TaskNotFound(format!(
                        "{}:{}",
                        task_id, batch_id
                    )));
                }
            };

            let result = collector.write().await.submit_result(msg.result).await;
            result
        };

        Box::pin(actix::fut::wrap_future(fut))
    }
}

impl Handler<GetVerificationStatus> for VerifierService {
    type Result = ResponseActFuture<Self, VerifierResult<VerificationResult>>;

    fn handle(&mut self, msg: GetVerificationStatus, _ctx: &mut Self::Context) -> Self::Result {
        let collectors = self.collectors.clone();

        let fut = async move {
            let collector = {
                let collectors = collectors.read().await;
                let key = VerifierService::get_collector_key(&msg.task_id, &msg.batch_id);
                collectors.get(&key).cloned()
            };

            let collector = match collector {
                Some(c) => c,
                None => {
                    return Err(VerifierError::TaskNotFound(format!(
                        "{}:{}",
                        msg.task_id, msg.batch_id
                    )));
                }
            };

            let status = collector.read().await.get_status().await;
            
            // If verification succeeded, generate offense contexts and slashing actions
            if let VerificationResult::Verified {
                ref malicious_providers,
                ref consensus_result_hash,
                ..
            } = status {
                if !malicious_providers.is_empty() {
                    // Generate offense contexts for severity evaluation
                    let contexts = collector
                        .read()
                        .await
                        .generate_offense_contexts(malicious_providers, consensus_result_hash)
                        .await;
                    
                    log::info!(
                        "Generated {} offense contexts for {} malicious providers",
                        contexts.len(),
                        malicious_providers.len()
                    );
                }
            }
            
            Ok(status)
        };

        Box::pin(actix::fut::wrap_future(fut))
    }
}

impl Handler<WaitForVerification> for VerifierService {
    type Result = ResponseActFuture<Self, VerifierResult<VerificationResult>>;

    fn handle(&mut self, msg: WaitForVerification, _ctx: &mut Self::Context) -> Self::Result {
        let collectors = self.collectors.clone();
        let timeout = msg.timeout.unwrap_or(Duration::seconds(60));

        let fut = async move {
            let collector = {
                let collectors = collectors.read().await;
                let key = VerifierService::get_collector_key(&msg.task_id, &msg.batch_id);
                collectors.get(&key).cloned()
            };

            let collector = match collector {
                Some(c) => c,
                None => {
                    return Err(VerifierError::TaskNotFound(format!(
                        "{}:{}",
                        msg.task_id, msg.batch_id
                    )));
                }
            };

            let start_time = Utc::now();
            let check_interval = Duration::milliseconds(100);

            loop {
                let status = collector.read().await.get_status().await;

                match &status {
                    VerificationResult::Verified { .. } | VerificationResult::Failed { .. } => {
                        return Ok(status);
                    }
                    VerificationResult::Pending { .. } => {
                        let elapsed = Utc::now() - start_time;
                        if elapsed > timeout {
                            return Err(VerifierError::Timeout(format!(
                                "Timeout waiting for verification after {:?}",
                                elapsed
                            )));
                        }
                    }
                }

                tokio::time::sleep(check_interval.to_std().unwrap()).await;
            }
        };

        Box::pin(actix::fut::wrap_future(fut))
    }
}

/// Get slashing actions for malicious providers (backward compatible)
pub async fn get_slashing_actions(
    malicious_providers: &[NodeId],
    offense_tracker: &Arc<RwLock<OffenseTracker>>,
) -> Vec<SlashingAction> {
    let mut tracker = offense_tracker.write().await;
    generate_slashing_actions(malicious_providers, &mut tracker)
}

/// Get slashing actions with severity evaluation
pub async fn get_slashing_actions_with_severity(
    malicious_providers: &[NodeId],
    offense_tracker: &Arc<RwLock<OffenseTracker>>,
    severity_evaluator: &Arc<RwLock<SeverityEvaluator>>,
    offense_contexts: &[OffenseContext],
) -> Vec<SlashingAction> {
    let mut tracker = offense_tracker.write().await;
    let mut evaluator = severity_evaluator.write().await;
    generate_slashing_actions_with_severity(
        malicious_providers,
        &mut tracker,
        &mut evaluator,
        offense_contexts,
    )
}

/// Get slashing actions with full context (stake amounts, severity, etc.)
pub async fn get_slashing_actions_advanced(
    malicious_providers: &[NodeId],
    offense_tracker: &Arc<RwLock<OffenseTracker>>,
    severity_evaluator: &Arc<RwLock<SeverityEvaluator>>,
    offense_contexts: &[OffenseContext],
    stake_amounts: &HashMap<NodeId, f64>,
) -> Vec<SlashingAction> {
    let mut tracker = offense_tracker.write().await;
    let mut evaluator = severity_evaluator.write().await;
    generate_slashing_actions_advanced(
        malicious_providers,
        &mut tracker,
        &mut evaluator,
        offense_contexts,
        stake_amounts,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn node_id(s: &str) -> NodeId {
        NodeId::from_str(s).unwrap()
    }

    #[actix_rt::test]
    async fn test_register_and_submit() {
        let service = VerifierService::new(None);
        let addr = service.start();

        // Register task
        let register = RegisterTask {
            task_id: "task1".to_string(),
            batch_id: "batch1".to_string(),
            expected_providers: 3,
            timeout: Some(Duration::seconds(60)),
        };
        addr.send(register).await.unwrap().unwrap();

        // Submit result
        let result = ProviderResult {
            provider_id: node_id("provider1"),
            task_id: "task1".to_string(),
            batch_id: "batch1".to_string(),
            results: vec![],
            timestamp: Utc::now(),
        };
        let submit = SubmitResult { result };
        addr.send(submit).await.unwrap().unwrap();

        // Get status
        let status_msg = GetVerificationStatus {
            task_id: "task1".to_string(),
            batch_id: "batch1".to_string(),
        };
        let status = addr.send(status_msg).await.unwrap().unwrap();
        match status {
            VerificationResult::Pending { received, .. } => {
                assert_eq!(received, 1);
            }
            _ => panic!("Expected pending status"),
        }
    }
}

