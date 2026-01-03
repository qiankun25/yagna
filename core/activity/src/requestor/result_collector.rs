//! Result collector for gathering results from multiple providers
//! 
//! This module handles asynchronous collection of results from multiple providers
//! with timeout management and error handling.

use anyhow::{anyhow, Result};
use chrono::Utc;
use futures::future::join_all;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;
use ya_client_model::activity::{ExeScriptCommand, ExeScriptCommandResult};
use ya_client_model::market::Agreement;
use ya_client_model::NodeId;
use ya_core_model::activity;
use ya_net::{self as net, RemoteEndpoint};
use ya_service_bus::{timeout::IntoTimeoutFuture, RpcEndpoint};
use ya_verifier::{
    consensus::{self, ConsensusEngine, ResultHash},
    result_collector::ProviderResult as VerifierProviderResult,
    VerificationResult as VerifierVerificationResult,
};

/// Configuration for result collection
#[derive(Debug, Clone)]
pub struct CollectorConfig {
    /// Timeout for collecting results from all providers
    pub collection_timeout: Duration,
    /// Timeout for individual provider requests
    pub provider_timeout: Duration,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            collection_timeout: Duration::from_secs(300), // 5 minutes
            provider_timeout: Duration::from_secs(60),    // 1 minute per provider
        }
    }
}

/// Result collector for redundant execution
pub struct ResultCollector {
    config: CollectorConfig,
}

impl ResultCollector {
    /// Create a new result collector with default configuration
    pub fn new() -> Self {
        Self {
            config: CollectorConfig::default(),
        }
    }

    /// Create a new result collector with custom configuration
    pub fn with_config(config: CollectorConfig) -> Self {
        Self { config }
    }

    /// Execute task on multiple providers and collect results
    /// 
    /// # Arguments
    /// * `agreements` - List of agreements with different providers
    /// * `activity_ids` - Map of agreement_id to activity_id for each provider
    /// * `batch_id` - The batch ID for this execution
    /// * `exe_script` - The execution script to run
    /// * `requestor_id` - The requestor's node ID
    /// 
    /// # Returns
    /// * `VerifierVerificationResult` - The verification outcome
    pub async fn collect_and_verify(
        &self,
        agreements: Vec<Agreement>,
        activity_ids: HashMap<String, String>,
        batch_id: String,
        exe_script: Vec<ExeScriptCommand>,
        requestor_id: NodeId,
    ) -> Result<VerifierVerificationResult> {
        log::info!(
            "Starting redundant execution: {} providers, batch_id: {}",
            agreements.len(),
            batch_id
        );

        // Execute on all providers concurrently
        let futures: Vec<_> = agreements
            .iter()
            .filter_map(|agreement| {
                let agreement_id = agreement.agreement_id.clone();
                let activity_id = match activity_ids.get(&agreement_id) {
                    Some(id) => id.clone(),
                    None => {
                        log::warn!("Activity ID not found for agreement {}", agreement_id);
                        return None;
                    }
                };
                let provider_id = *agreement.provider_id();
                let batch_id = batch_id.clone();
                let exe_script = exe_script.clone();

                Some(async move {
                    self.collect_from_provider(
                        requestor_id,
                        provider_id,
                        activity_id,
                        batch_id,
                        exe_script,
                    )
                    .await
                    .map(|result| (provider_id, result))
                })
            })
            .collect();

        // Wait for all results with timeout
        let collection_result = timeout(self.config.collection_timeout, join_all(futures)).await;

        let provider_results = match collection_result {
            Ok(results) => {
                // Filter out errors and collect successful results
                let mut collected = Vec::new();
                for result in results {
                    match result {
                        Ok((provider_id, Some(results))) => {
                            collected.push(VerifierProviderResult {
                                provider_id,
                                task_id: format!("task-{}", batch_id),
                                batch_id: batch_id.clone(),
                                results,
                                timestamp: Utc::now(),
                            });
                        }
                        Ok((provider_id, None)) => {
                            log::warn!("Provider {} returned no results", provider_id);
                        }
                        Err(e) => {
                            log::warn!("Failed to collect from provider: {}", e);
                        }
                    }
                }
                collected
            }
            Err(_) => {
                log::error!("Timeout waiting for results from providers");
                return Ok(VerifierVerificationResult::Pending {
                    received: 0,
                    required: ConsensusEngine::min_votes_required(agreements.len()),
                });
            }
        };

        // Verify the collected results using ya-verifier's consensus engine
        Ok(self.verify_results(provider_results))
    }

    /// Collect results from a single provider
    async fn collect_from_provider(
        &self,
        requestor_id: NodeId,
        provider_id: NodeId,
        activity_id: String,
        batch_id: String,
        exe_script: Vec<ExeScriptCommand>,
    ) -> Result<Option<Vec<ExeScriptCommandResult>>> {
        log::debug!(
            "Collecting results from provider {} for activity {}",
            provider_id,
            activity_id
        );

        // Send execution request
        let exec_msg = activity::Exec {
            activity_id: activity_id.clone(),
            batch_id: batch_id.clone(),
            exe_script: exe_script.clone(),
            timeout: None,
        };

        // Send exec command
        let _exec_result = net::from(requestor_id)
            .to(provider_id)
            .service(&activity::exeunit::bus_id(&activity_id))
            .send(exec_msg)
            .timeout(Some(self.config.provider_timeout))
            .await
            .map_err(|e| anyhow!("Failed to send exec to provider {}: {}", provider_id, e))??;

        // Wait a bit for execution to complete
        // In a real implementation, we would poll or use events
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Get results
        let get_results_msg = activity::GetExecBatchResults {
            activity_id,
            batch_id,
            timeout: Some(self.config.provider_timeout.as_secs() as f32),
            command_index: None,
        };

        let results = net::from(requestor_id)
            .to(provider_id)
            .service_transfer(&activity::exeunit::bus_id(&get_results_msg.activity_id))
            .send(get_results_msg)
            .timeout(Some(self.config.provider_timeout))
            .await
            .map_err(|e| anyhow!("Failed to get results from provider {}: {}", provider_id, e))??;

        Ok(Some(results?))
    }

    /// Verify collected results using ya-verifier's consensus engine
    fn verify_results(&self, provider_results: Vec<VerifierProviderResult>) -> VerifierVerificationResult {
        if provider_results.is_empty() {
            return VerifierVerificationResult::Pending {
                received: 0,
                required: 1,
            };
        }

        let total_providers = provider_results.len();
        let _min_votes = ConsensusEngine::min_votes_required(total_providers);

        // Convert provider results to result hashes
        let mut result_hashes: HashMap<NodeId, ResultHash> = HashMap::new();
        let mut result_data: HashMap<String, Vec<u8>> = HashMap::new();

        for provider_result in &provider_results {
            // Serialize result to bytes for hashing
            let result_bytes = match serde_json::to_vec(&provider_result.results) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::warn!("Failed to serialize result from {}: {}", provider_result.provider_id, e);
                    continue;
                }
            };

            // Calculate hash
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&result_bytes);
            let hash = hex::encode(hasher.finalize());

            result_hashes.insert(
                provider_result.provider_id,
                ResultHash {
                    hash: hash.clone(),
                    data: result_bytes.clone(),
                },
            );
            result_data.insert(hash, result_bytes);
        }

        // Run consensus algorithm
        match ConsensusEngine::reach_consensus(&result_hashes, &result_data) {
            Ok(consensus::ConsensusOutcome::Consensus {
                result,
                votes,
                total,
            }) => {
                // Deserialize the consensus result
                let verified_results: Vec<ExeScriptCommandResult> =
                    match serde_json::from_slice(&result.data) {
                        Ok(results) => results,
                        Err(e) => {
                            log::error!("Failed to deserialize consensus result: {}", e);
                            return VerifierVerificationResult::Failed {
                                distribution: HashMap::new(),
                                all_results: provider_results,
                            };
                        }
                    };

                // Identify honest and malicious providers
                let honest_providers: Vec<NodeId> = result_hashes
                    .iter()
                    .filter_map(|(id, hash)| {
                        if hash.hash == result.hash {
                            Some(id.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                let malicious_providers = ConsensusEngine::identify_malicious_providers(
                    &result_hashes,
                    &result.hash,
                );

                VerifierVerificationResult::Verified {
                    result: verified_results,
                    votes,
                    total,
                    honest_providers,
                    malicious_providers,
                    consensus_result_hash: result.hash,
                }
            }
            Ok(consensus::ConsensusOutcome::NoConsensus { distribution }) => {
                VerifierVerificationResult::Failed {
                    distribution,
                    all_results: provider_results,
                }
            }
            Err(e) => {
                log::error!("Consensus error: {:?}", e);
                VerifierVerificationResult::Failed {
                    distribution: HashMap::new(),
                    all_results: provider_results,
                }
            }
        }
    }
}

impl Default for ResultCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collector_creation() {
        let collector = ResultCollector::new();
        assert_eq!(
            collector.config.collection_timeout,
            Duration::from_secs(300)
        );
    }
}

