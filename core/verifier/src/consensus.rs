//! Simplified BFT consensus algorithm implementation
//!
//! This module implements a simplified Byzantine Fault Tolerant consensus
//! algorithm that requires 2/3 majority agreement to accept a result.

use crate::error::{VerifierError, VerifierResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ya_client_model::NodeId;

/// Represents a result submitted by a provider
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResultHash {
    /// Hash of the result data
    pub hash: String,
    /// The actual result data (for comparison)
    pub data: Vec<u8>,
}

/// Result of consensus voting
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsensusOutcome {
    /// Consensus reached with majority result
    Consensus {
        /// The agreed-upon result
        result: ResultHash,
        /// Number of providers that agreed
        votes: usize,
        /// Total number of providers
        total: usize,
    },
    /// No consensus reached
    NoConsensus {
        /// Distribution of results
        distribution: HashMap<String, usize>,
    },
}

/// Simplified BFT consensus algorithm
///
/// This algorithm requires at least 2/3 of providers to agree on a result
/// for it to be considered valid. This provides Byzantine fault tolerance
/// assuming less than 1/3 of providers are malicious.
pub struct ConsensusEngine;

impl ConsensusEngine {
    /// Calculate the minimum number of votes needed for consensus
    ///
    /// For BFT, we need more than 2/3 of total providers to agree.
    /// Formula: floor(total * 2/3) + 1
    pub fn min_votes_required(total_providers: usize) -> usize {
        if total_providers == 0 {
            return 0;
        }
        // Need more than 2/3, so we use: floor(2/3 * n) + 1
        // This ensures we have at least 2/3 + 1 votes
        (total_providers * 2) / 3 + 1
    }

    /// Check if we have enough votes for consensus
    pub fn has_consensus(votes: usize, total_providers: usize) -> bool {
        if total_providers == 0 {
            return false;
        }
        votes >= Self::min_votes_required(total_providers)
    }

    /// Run consensus algorithm on collected results
    ///
    /// # Arguments
    /// * `results` - Map from provider ID to their result hash
    /// * `result_data` - Map from result hash to actual result data
    ///
    /// # Returns
    /// Consensus outcome indicating whether consensus was reached
    pub fn reach_consensus(
        results: &HashMap<NodeId, ResultHash>,
        result_data: &HashMap<String, Vec<u8>>,
    ) -> VerifierResult<ConsensusOutcome> {
        if results.is_empty() {
            return Err(VerifierError::InsufficientResults {
                received: 0,
                required: 1,
            });
        }

        let total_providers = results.len();
        let min_votes = Self::min_votes_required(total_providers);

        // Count votes for each result hash
        let mut vote_counts: HashMap<String, usize> = HashMap::new();
        for result_hash in results.values() {
            *vote_counts.entry(result_hash.hash.clone()).or_insert(0) += 1;
        }

        // Find the result with the most votes
        let (winning_hash, votes) = vote_counts
            .iter()
            .max_by_key(|(_, &count)| count)
            .ok_or_else(|| VerifierError::Internal("No votes found".to_string()))?;

        log::debug!(
            "Consensus check: {} votes for hash {}, need {} out of {} providers",
            votes,
            winning_hash,
            min_votes,
            total_providers
        );

        // Check if we have consensus (>= 2/3 majority)
        if *votes >= min_votes {
            let result_data = result_data
                .get(winning_hash)
                .ok_or_else(|| {
                    VerifierError::Internal(format!("Result data not found for hash {}", winning_hash))
                })?
                .clone();

            Ok(ConsensusOutcome::Consensus {
                result: ResultHash {
                    hash: winning_hash.clone(),
                    data: result_data,
                },
                votes: *votes,
                total: total_providers,
            })
        } else {
            // No consensus reached
            Ok(ConsensusOutcome::NoConsensus {
                distribution: vote_counts,
            })
        }
    }

    /// Identify malicious providers based on consensus outcome
    ///
    /// Returns a list of provider IDs that submitted results different from the consensus
    pub fn identify_malicious_providers(
        results: &HashMap<NodeId, ResultHash>,
        consensus_hash: &str,
    ) -> Vec<NodeId> {
        results
            .iter()
            .filter_map(|(provider_id, result_hash)| {
                if result_hash.hash != consensus_hash {
                    Some(provider_id.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn node_id(s: &str) -> NodeId {
        NodeId::from_str(s).unwrap()
    }

    #[test]
    fn test_min_votes_required() {
        assert_eq!(ConsensusEngine::min_votes_required(3), 3); // 2/3 of 3 = 2, need 3
        assert_eq!(ConsensusEngine::min_votes_required(4), 3); // 2/3 of 4 = 2.67, need 3
        assert_eq!(ConsensusEngine::min_votes_required(5), 4); // 2/3 of 5 = 3.33, need 4
        assert_eq!(ConsensusEngine::min_votes_required(6), 5); // 2/3 of 6 = 4, need 5
        assert_eq!(ConsensusEngine::min_votes_required(9), 7); // 2/3 of 9 = 6, need 7
    }

    #[test]
    fn test_consensus_with_majority() {
        let mut results = HashMap::new();
        let mut result_data = HashMap::new();

        let correct_hash = "hash1".to_string();
        let correct_data = b"correct_result".to_vec();
        result_data.insert(correct_hash.clone(), correct_data.clone());

        // 5 providers, need 4 for consensus
        // 4 providers agree
        for i in 0..4 {
            let provider_id = node_id(&format!("provider_{}", i));
            results.insert(
                provider_id,
                ResultHash {
                    hash: correct_hash.clone(),
                    data: correct_data.clone(),
                },
            );
        }

        // 1 provider disagrees
        let wrong_hash = "hash2".to_string();
        let wrong_data = b"wrong_result".to_vec();
        result_data.insert(wrong_hash.clone(), wrong_data.clone());
        results.insert(
            node_id("provider_4"),
            ResultHash {
                hash: wrong_hash,
                data: wrong_data,
            },
        );

        let outcome = ConsensusEngine::reach_consensus(&results, &result_data).unwrap();
        match outcome {
            ConsensusOutcome::Consensus { result, votes, total } => {
                assert_eq!(result.hash, correct_hash);
                assert_eq!(votes, 4);
                assert_eq!(total, 5);
            }
            _ => panic!("Expected consensus"),
        }
    }

    #[test]
    fn test_no_consensus() {
        let mut results = HashMap::new();
        let mut result_data = HashMap::new();

        let hash1 = "hash1".to_string();
        let hash2 = "hash2".to_string();
        let hash3 = "hash3".to_string();

        result_data.insert(hash1.clone(), b"result1".to_vec());
        result_data.insert(hash2.clone(), b"result2".to_vec());
        result_data.insert(hash3.clone(), b"result3".to_vec());

        // 3 providers, need 3 for consensus
        // But we have 3 different results (no majority)
        results.insert(
            node_id("provider_1"),
            ResultHash {
                hash: hash1.clone(),
                data: b"result1".to_vec(),
            },
        );
        results.insert(
            node_id("provider_2"),
            ResultHash {
                hash: hash2.clone(),
                data: b"result2".to_vec(),
            },
        );
        results.insert(
            node_id("provider_3"),
            ResultHash {
                hash: hash3.clone(),
                data: b"result3".to_vec(),
            },
        );

        let outcome = ConsensusEngine::reach_consensus(&results, &result_data).unwrap();
        match outcome {
            ConsensusOutcome::NoConsensus { distribution } => {
                assert_eq!(distribution.len(), 3);
                assert_eq!(distribution.get(&hash1), Some(&1));
                assert_eq!(distribution.get(&hash2), Some(&1));
                assert_eq!(distribution.get(&hash3), Some(&1));
            }
            _ => panic!("Expected no consensus"),
        }
    }

    #[test]
    fn test_identify_malicious_providers() {
        let mut results = HashMap::new();
        let correct_hash = "correct_hash".to_string();

        // 3 honest providers
        for i in 0..3 {
            results.insert(
                node_id(&format!("honest_{}", i)),
                ResultHash {
                    hash: correct_hash.clone(),
                    data: vec![],
                },
            );
        }

        // 2 malicious providers
        results.insert(
            node_id("malicious_1"),
            ResultHash {
                hash: "wrong_hash_1".to_string(),
                data: vec![],
            },
        );
        results.insert(
            node_id("malicious_2"),
            ResultHash {
                hash: "wrong_hash_2".to_string(),
                data: vec![],
            },
        );

        let malicious = ConsensusEngine::identify_malicious_providers(&results, &correct_hash);
        assert_eq!(malicious.len(), 2);
        assert!(malicious.contains(&node_id("malicious_1")));
        assert!(malicious.contains(&node_id("malicious_2")));
    }
}

