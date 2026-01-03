//! Result validation module
//!
//! This module provides validation logic for provider results based on the
//! data model defined in provider_results_data_model.md. It validates:
//! - Result format and completeness
//! - Resource usage consistency
//! - Cost calculation correctness
//! - Timestamp validity

use crate::slashing::OffenseType;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use ya_client_model::activity::ExeScriptCommandResult;

/// Validation result for a single provider's result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the result is valid
    pub is_valid: bool,
    /// Detected offense types (empty if valid)
    pub offenses: Vec<OffenseType>,
    /// Validation errors/warnings
    pub errors: Vec<String>,
}

/// Validator for provider results
pub struct ResultValidator {
    /// Tolerance for resource usage differences (e.g., 0.1 = 10%)
    pub resource_tolerance: f64,
    /// Tolerance for cost differences (e.g., 0.05 = 5%)
    pub cost_tolerance: f64,
    /// Maximum allowed timestamp difference in seconds
    pub max_timestamp_diff_secs: i64,
}

impl Default for ResultValidator {
    fn default() -> Self {
        Self {
            resource_tolerance: 0.1, // 10% tolerance
            cost_tolerance: 0.05,     // 5% tolerance
            max_timestamp_diff_secs: 300, // 5 minutes
        }
    }
}

impl ResultValidator {
    /// Create a new validator with custom tolerances
    pub fn new(
        resource_tolerance: f64,
        cost_tolerance: f64,
        max_timestamp_diff_secs: i64,
    ) -> Self {
        Self {
            resource_tolerance,
            cost_tolerance,
            max_timestamp_diff_secs,
        }
    }

    /// Validate result format and completeness
    pub fn validate_format(&self, results: &[ExeScriptCommandResult]) -> ValidationResult {
        let mut offenses = Vec::new();
        let mut errors = Vec::new();

        // Check for empty results
        if results.is_empty() {
            errors.push("Empty result set".to_string());
            offenses.push(OffenseType::FormatViolation {
                violation: "Empty result set".to_string(),
            });
            return ValidationResult {
                is_valid: false,
                offenses,
                errors,
            };
        }

        // Check index continuity
        let mut indices: Vec<u32> = results.iter().map(|r| r.index).collect();
        indices.sort();
        
        for (i, &idx) in indices.iter().enumerate() {
            if idx != i as u32 {
                let violation = format!("Index discontinuity: expected {}, got {}", i, idx);
                errors.push(violation.clone());
                offenses.push(OffenseType::FormatViolation { violation });
            }
        }

        // Check for duplicate indices
        let mut seen = std::collections::HashSet::new();
        for result in results {
            if !seen.insert(result.index) {
                let violation = format!("Duplicate index: {}", result.index);
                errors.push(violation.clone());
                offenses.push(OffenseType::FormatViolation { violation });
            }
        }

        // Check timestamp validity (not in the future)
        let now = Utc::now();
        for result in results {
            if result.event_date > now {
                let time_diff = (result.event_date - now).num_seconds();
                offenses.push(OffenseType::TimestampAnomaly {
                    reported_time: result.event_date,
                    expected_time: now,
                    time_difference_secs: time_diff,
                });
                errors.push(format!(
                    "Future timestamp detected: {} seconds in the future",
                    time_diff
                ));
            }
        }

        ValidationResult {
            is_valid: offenses.is_empty(),
            offenses,
            errors,
        }
    }

    /// Validate resource usage against consensus
    pub fn validate_resource_usage(
        &self,
        reported_usage: &HashMap<String, f64>,
        consensus_usage: &HashMap<String, f64>,
    ) -> ValidationResult {
        let mut offenses = Vec::new();
        let mut errors = Vec::new();

        for (resource_type, &reported) in reported_usage.iter() {
            if let Some(&expected) = consensus_usage.get(resource_type) {
                let diff_ratio = if expected > 0.0 {
                    (reported - expected).abs() / expected
                } else if reported > 0.0 {
                    // Expected is 0 but reported is not - this is inflation
                    f64::INFINITY
                } else {
                    0.0
                };

                if diff_ratio > self.resource_tolerance {
                    let inflation_ratio = if expected > 0.0 {
                        reported / expected
                    } else {
                        f64::INFINITY
                    };

                    offenses.push(OffenseType::ResourceInflation {
                        resource_type: resource_type.clone(),
                        reported_usage: reported,
                        expected_usage: expected,
                        inflation_ratio,
                    });

                    errors.push(format!(
                        "Resource inflation detected for {}: reported {}, expected {}, ratio: {:.2}",
                        resource_type, reported, expected, inflation_ratio
                    ));
                }
            } else {
                // Resource type not in consensus - might be new or invalid
                errors.push(format!(
                    "Unknown resource type in usage: {}",
                    resource_type
                ));
            }
        }

        ValidationResult {
            is_valid: offenses.is_empty(),
            offenses,
            errors,
        }
    }

    /// Validate cost calculation against consensus
    pub fn validate_cost(
        &self,
        reported_cost: f64,
        consensus_cost: f64,
    ) -> ValidationResult {
        let mut offenses = Vec::new();
        let mut errors = Vec::new();

        let cost_diff = (reported_cost - consensus_cost).abs();
        let cost_diff_ratio = if consensus_cost > 0.0 {
            cost_diff / consensus_cost
        } else if reported_cost > 0.0 {
            // Consensus cost is 0 but reported is not
            f64::INFINITY
        } else {
            0.0
        };

        if cost_diff_ratio > self.cost_tolerance {
            offenses.push(OffenseType::CostFraud {
                reported_cost,
                expected_cost: consensus_cost,
                cost_difference: cost_diff,
            });

            errors.push(format!(
                "Cost fraud detected: reported {}, expected {}, difference: {:.6}, ratio: {:.2}%",
                reported_cost, consensus_cost, cost_diff, cost_diff_ratio * 100.0
            ));
        }

        ValidationResult {
            is_valid: offenses.is_empty(),
            offenses,
            errors,
        }
    }

    /// Validate timestamp consistency
    pub fn validate_timestamps(
        &self,
        reported_timestamps: &[DateTime<Utc>],
        consensus_timestamp: DateTime<Utc>,
    ) -> ValidationResult {
        let mut offenses = Vec::new();
        let mut errors = Vec::new();

        for &reported_time in reported_timestamps {
            let time_diff = (reported_time - consensus_timestamp).num_seconds().abs();
            
            if time_diff > self.max_timestamp_diff_secs {
                offenses.push(OffenseType::TimestampAnomaly {
                    reported_time,
                    expected_time: consensus_timestamp,
                    time_difference_secs: time_diff,
                });

                errors.push(format!(
                    "Timestamp anomaly: {} seconds difference (max allowed: {})",
                    time_diff, self.max_timestamp_diff_secs
                ));
            }
        }

        ValidationResult {
            is_valid: offenses.is_empty(),
            offenses,
            errors,
        }
    }

    /// Comprehensive validation combining all checks
    pub fn validate_comprehensive(
        &self,
        results: &[ExeScriptCommandResult],
        consensus_results: &[ExeScriptCommandResult],
        reported_usage: Option<&HashMap<String, f64>>,
        consensus_usage: Option<&HashMap<String, f64>>,
        reported_cost: Option<f64>,
        consensus_cost: Option<f64>,
    ) -> ValidationResult {
        let mut all_offenses = Vec::new();
        let mut all_errors = Vec::new();

        // Format validation
        let format_result = self.validate_format(results);
        all_offenses.extend(format_result.offenses);
        all_errors.extend(format_result.errors);

        // Resource usage validation
        if let (Some(reported), Some(consensus)) = (reported_usage, consensus_usage) {
            let usage_result = self.validate_resource_usage(reported, consensus);
            all_offenses.extend(usage_result.offenses);
            all_errors.extend(usage_result.errors);
        }

        // Cost validation
        if let (Some(reported), Some(consensus)) = (reported_cost, consensus_cost) {
            let cost_result = self.validate_cost(reported, consensus);
            all_offenses.extend(cost_result.offenses);
            all_errors.extend(cost_result.errors);
        }

        // Timestamp validation
        let reported_timestamps: Vec<DateTime<Utc>> = results.iter().map(|r| r.event_date).collect();
        if let Some(consensus_result) = consensus_results.first() {
            let timestamp_result = self.validate_timestamps(&reported_timestamps, consensus_result.event_date);
            all_offenses.extend(timestamp_result.offenses);
            all_errors.extend(timestamp_result.errors);
        }

        // Combine multiple offenses if needed
        let is_valid = all_offenses.is_empty();
        let final_offenses = if all_offenses.is_empty() {
            vec![OffenseType::WrongResult] // Default to wrong result if no specific offense detected
        } else if all_offenses.len() == 1 {
            all_offenses
        } else {
            vec![OffenseType::Multiple(all_offenses)]
        };

        ValidationResult {
            is_valid,
            offenses: final_offenses,
            errors: all_errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ya_client_model::activity::CommandResult;

    fn create_test_result(index: u32, event_date: DateTime<Utc>) -> ExeScriptCommandResult {
        ExeScriptCommandResult {
            index,
            result: CommandResult::Ok,
            stdout: None,
            stderr: None,
            message: None,
            is_batch_finished: false,
            event_date,
        }
    }

    #[test]
    fn test_format_validation_empty() {
        let validator = ResultValidator::default();
        let result = validator.validate_format(&[]);
        assert!(!result.is_valid);
        assert!(!result.offenses.is_empty());
    }

    #[test]
    fn test_format_validation_continuous() {
        let validator = ResultValidator::default();
        let now = Utc::now();
        let results = vec![
            create_test_result(0, now),
            create_test_result(1, now),
            create_test_result(2, now),
        ];
        let result = validator.validate_format(&results);
        assert!(result.is_valid);
    }

    #[test]
    fn test_format_validation_discontinuous() {
        let validator = ResultValidator::default();
        let now = Utc::now();
        let results = vec![
            create_test_result(0, now),
            create_test_result(2, now), // Missing index 1
        ];
        let result = validator.validate_format(&results);
        assert!(!result.is_valid);
    }

    #[test]
    fn test_resource_validation() {
        let validator = ResultValidator::default();
        let mut reported = HashMap::new();
        reported.insert("cpu".to_string(), 20.0);
        reported.insert("mem".to_string(), 150.0);

        let mut consensus = HashMap::new();
        consensus.insert("cpu".to_string(), 10.0); // 100% inflation
        consensus.insert("mem".to_string(), 140.0); // Within tolerance

        let result = validator.validate_resource_usage(&reported, &consensus);
        assert!(!result.is_valid);
        assert!(result.offenses.iter().any(|o| matches!(
            o,
            OffenseType::ResourceInflation { resource_type, .. } if resource_type == "cpu"
        )));
    }

    #[test]
    fn test_cost_validation() {
        let validator = ResultValidator::default();
        let result = validator.validate_cost(1.1, 1.0); // 10% difference, exceeds 5% tolerance
        assert!(!result.is_valid);
        assert!(result.offenses.iter().any(|o| matches!(
            o,
            OffenseType::CostFraud { .. }
        )));
    }
}

