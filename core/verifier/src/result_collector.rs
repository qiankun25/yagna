//! Result collector for gathering results from multiple providers

use crate::consensus::{ConsensusEngine, ConsensusOutcome, ResultHash};
use crate::error::{VerifierError, VerifierResult};
use crate::result_comparator::ResultComparator;
use crate::result_validator::ResultValidator;
use crate::slashing::{OffenseContext, OffenseType};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use ya_client_model::{activity::ExeScriptCommandResult, NodeId};

/// Result submitted by a single provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResult {
    /// Provider node ID
    pub provider_id: NodeId,
    /// Task ID this result is for
    pub task_id: String,
    /// Batch ID
    pub batch_id: String,
    /// Execution results
    pub results: Vec<ExeScriptCommandResult>,
    /// Timestamp when result was submitted
    pub timestamp: DateTime<Utc>,
}

/// Final verification result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VerificationResult {
    /// Verification successful with consensus
    Verified {
        /// The verified result
        result: Vec<ExeScriptCommandResult>,
        /// Number of providers that agreed
        votes: usize,
        /// Total number of providers
        total: usize,
        /// Providers that submitted the correct result
        honest_providers: Vec<NodeId>,
        /// Providers that submitted incorrect results (malicious)
        malicious_providers: Vec<NodeId>,
        /// Hash of the consensus result (for offense context generation)
        consensus_result_hash: String,
    },
    /// Verification failed - no consensus
    Failed {
        /// Distribution of results
        distribution: HashMap<String, usize>,
        /// All provider results
        all_results: Vec<ProviderResult>,
        /// Detected malicious providers (even without consensus)
        detected_malicious: Vec<NodeId>,
    },
    /// Verification pending - waiting for more results
    Pending {
        /// Number of results received so far
        received: usize,
        /// Minimum number required
        required: usize,
    },
}

/// Collects and verifies results from multiple providers
pub struct ResultCollector {
    /// Task ID being verified
    task_id: String,
    /// Batch ID
    batch_id: String,
    /// Expected number of providers
    expected_providers: usize,
    /// Collected results from providers
    results: Arc<RwLock<HashMap<NodeId, ProviderResult>>>,
    /// Timeout for collecting results
    timeout: Option<chrono::Duration>,
    /// Start time of collection
    start_time: DateTime<Utc>,
}

impl ResultCollector {
    /// Create a new result collector
    pub fn new(
        task_id: String,
        batch_id: String,
        expected_providers: usize,
        timeout: Option<chrono::Duration>,
    ) -> Self {
        Self {
            task_id,
            batch_id,
            expected_providers,
            results: Arc::new(RwLock::new(HashMap::new())),
            timeout,
            start_time: Utc::now(),
        }
    }

    /// Submit a result from a provider
    pub async fn submit_result(&self, result: ProviderResult) -> VerifierResult<()> {
        // Validate task and batch IDs
        if result.task_id != self.task_id {
            return Err(VerifierError::InvalidResult(format!(
                "Task ID mismatch: expected {}, got {}",
                self.task_id, result.task_id
            )));
        }

        if result.batch_id != self.batch_id {
            return Err(VerifierError::InvalidResult(format!(
                "Batch ID mismatch: expected {}, got {}",
                self.batch_id, result.batch_id
            )));
        }

        let mut results = self.results.write().await;

        // Check for duplicate
        if results.contains_key(&result.provider_id) {
            return Err(VerifierError::DuplicateProvider(
                result.provider_id.to_string(),
            ));
        }

        results.insert(result.provider_id.clone(), result);
        Ok(())
    }

    /// Check if we have enough results to attempt consensus
    pub async fn has_sufficient_results(&self) -> bool {
        let results = self.results.read().await;
        let received = results.len();
        let required = ConsensusEngine::min_votes_required(self.expected_providers);
        received >= required
    }

    /// Check if timeout has been exceeded
    pub fn is_timeout(&self) -> bool {
        if let Some(timeout) = self.timeout {
            let elapsed = Utc::now() - self.start_time;
            elapsed > timeout
        } else {
            false
        }
    }

    /// Get current verification status
    pub async fn get_status(&self) -> VerificationResult {
        let results = self.results.read().await;
        let received = results.len();
        let required = ConsensusEngine::min_votes_required(self.expected_providers);

        // IMPORTANT: We must wait for at least expected_providers results before attempting consensus
        // This prevents collusion attacks where malicious nodes submit early and reach consensus
        // before honest nodes can submit their results
        if received < self.expected_providers {
            return VerificationResult::Pending {
                received,
                required: self.expected_providers,
            };
        }

        // Now check if we have enough votes for consensus
        if received < required {
            return VerificationResult::Pending {
                received,
                required,
            };
        }

        // Try to reach consensus
        self.verify_internal(&results).await
    }

    /// Perform verification with current results
    async fn verify_internal(
        &self,
        results: &HashMap<NodeId, ProviderResult>,
    ) -> VerificationResult {
        if results.is_empty() {
            return VerificationResult::Pending {
                received: 0,
                required: ConsensusEngine::min_votes_required(self.expected_providers),
            };
        }

        // Convert provider results to result hashes
        let mut result_hashes: HashMap<NodeId, ResultHash> = HashMap::new();
        let mut result_data: HashMap<String, Vec<u8>> = HashMap::new();

        for (provider_id, provider_result) in results.iter() {
            // Serialize result to bytes for hashing
            let result_bytes = match serde_json::to_vec(&provider_result.results) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::warn!("Failed to serialize result from {}: {}", provider_id, e);
                    continue;
                }
            };

            // Calculate hash
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&result_bytes);
            let hash = hex::encode(hasher.finalize());

            result_hashes.insert(
                provider_id.clone(),
                ResultHash {
                    hash: hash.clone(),
                    data: result_bytes.clone(),
                },
            );
            result_data.insert(hash, result_bytes);
        }

        // Run consensus algorithm
        match ConsensusEngine::reach_consensus(&result_hashes, &result_data) {
            Ok(ConsensusOutcome::Consensus {
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
                        return VerificationResult::Failed {
                            distribution: HashMap::new(),
                            all_results: results.values().cloned().collect(),
                            detected_malicious: vec![],
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

                let malicious_providers: Vec<NodeId> = result_hashes
                    .iter()
                    .filter_map(|(id, hash)| {
                        if hash.hash != result.hash {
                            Some(id.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                VerificationResult::Verified {
                    result: verified_results,
                    votes,
                    total,
                    honest_providers,
                    malicious_providers,
                    consensus_result_hash: result.hash.clone(),
                }
            }
            Ok(ConsensusOutcome::NoConsensus { distribution }) => {
                // Even without consensus, we can detect malicious providers
                // by comparing results command by command
                let all_results_vec: Vec<Vec<ExeScriptCommandResult>> = results
                    .values()
                    .map(|pr| pr.results.clone())
                    .collect();
                
                // Detect partial tampering
                let tampered_indices = ResultComparator::detect_partial_tampering(&all_results_vec, 0.1);
                
                // Convert indices to provider IDs
                let provider_ids: Vec<NodeId> = results.keys().cloned().collect();
                let mut detected_malicious = Vec::new();
                
                for &idx in &tampered_indices {
                    if idx < provider_ids.len() {
                        detected_malicious.push(provider_ids[idx].clone());
                    }
                }
                
                // Also identify providers with completely different results
                // (those that don't match any majority result)
                if distribution.len() > 1 {
                    // Find the most common result hash
                    let max_votes = distribution.values().max().copied().unwrap_or(0);
                    let majority_hash = distribution
                        .iter()
                        .find(|(_, &votes)| votes == max_votes)
                        .map(|(hash, _)| hash.clone());
                    
                    if let Some(majority) = majority_hash {
                        // Find providers that don't match the majority
                        for (provider_id, result_hash) in &result_hashes {
                            if result_hash.hash != majority && !detected_malicious.contains(provider_id) {
                                detected_malicious.push(provider_id.clone());
                            }
                        }
                    }
                }
                
                VerificationResult::Failed {
                    distribution,
                    all_results: results.values().cloned().collect(),
                    detected_malicious,
                }
            }
            Err(e) => {
                log::error!("Consensus error: {}", e);
                VerificationResult::Failed {
                    distribution: HashMap::new(),
                    all_results: results.values().cloned().collect(),
                    detected_malicious: vec![],
                }
            }
        }
    }

    /// Get all collected results
    pub async fn get_all_results(&self) -> Vec<ProviderResult> {
        let results = self.results.read().await;
        results.values().cloned().collect()
    }

    /// Get number of results received
    pub async fn result_count(&self) -> usize {
        let results = self.results.read().await;
        results.len()
    }

    /// Generate offense contexts for malicious providers
    /// This is used for severity evaluation
    pub async fn generate_offense_contexts(
        &self,
        malicious_providers: &[NodeId],
        correct_result_hash: &str,
    ) -> Vec<OffenseContext> {
        let results = self.results.read().await;
        let mut contexts = Vec::new();

        // Group malicious providers by their result hash (to detect collusion)
        let mut result_hash_groups: HashMap<String, Vec<NodeId>> = HashMap::new();
        
        for provider_id in malicious_providers {
            if let Some(provider_result) = results.get(provider_id) {
                // Calculate hash of the wrong result
                let result_bytes = match serde_json::to_vec(&provider_result.results) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };

                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(&result_bytes);
                let wrong_hash = hex::encode(hasher.finalize());

                result_hash_groups
                    .entry(wrong_hash.clone())
                    .or_insert_with(Vec::new)
                    .push(provider_id.clone());
            }
        }

        // Create offense context for each malicious provider
        for provider_id in malicious_providers {
            if let Some(provider_result) = results.get(provider_id) {
                // Calculate hash of the wrong result
                let result_bytes = match serde_json::to_vec(&provider_result.results) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };

                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(&result_bytes);
                let wrong_hash = hex::encode(hasher.finalize());

                // Find colluding providers (those with the same wrong result)
                let colluding_providers: Vec<NodeId> = result_hash_groups
                    .get(&wrong_hash)
                    .map(|group| {
                        group
                            .iter()
                            .filter(|&id| id != provider_id)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();

                // Validate the result to detect specific offense types
                let validator = ResultValidator::default();
                let validation = validator.validate_format(&provider_result.results);
                
                // Determine offense type (use first detected offense or default to WrongResult)
                let offense_type = if !validation.offenses.is_empty() {
                    validation.offenses.first().unwrap().clone()
                } else {
                    OffenseType::WrongResult
                };

                contexts.push(OffenseContext {
                    provider_id: provider_id.clone(),
                    offense_time: provider_result.timestamp,
                    wrong_result_hash: wrong_hash,
                    correct_result_hash: correct_result_hash.to_string(),
                    offense_type,
                    colluding_providers,
                    total_providers: results.len(),
                    malicious_count: malicious_providers.len(),
                    resource_usage: None, // TODO: Extract from result if available
                    cost_info: None,      // TODO: Extract from result if available
                });
            }
        }

        contexts
    }
}

