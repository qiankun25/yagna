//! Slashing (penalty) mechanism for malicious providers
//!
//! This module implements an advanced slashing mechanism with:
//! - Multi-dimensional severity evaluation
//! - Proportional penalty calculation (based on stake amount)
//! - Historical offense tracking
//! - Collusion detection

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ya_client_model::NodeId;

/// Offense severity levels
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum OffenseSeverity {
    /// Minor offense: likely hardware failure or accidental error
    Minor,
    /// Moderate offense: repeated errors or obvious anomalies
    Moderate,
    /// Severe offense: collusion attack or systematic malicious behavior
    Severe,
}

/// Type of offense detected
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OffenseType {
    /// Wrong execution result (output mismatch)
    WrongResult,
    /// Resource usage inflation (exaggerated resource consumption)
    ResourceInflation {
        /// Resource type that was inflated (e.g., "cpu", "gpu", "mem")
        resource_type: String,
        /// Reported usage amount
        reported_usage: f64,
        /// Expected/consensus usage amount
        expected_usage: f64,
        /// Inflation ratio (reported / expected)
        inflation_ratio: f64,
    },
    /// Cost calculation fraud (incorrect cost computation)
    CostFraud {
        /// Reported cost
        reported_cost: f64,
        /// Expected/consensus cost
        expected_cost: f64,
        /// Cost difference
        cost_difference: f64,
    },
    /// Timestamp manipulation (future timestamps or unreasonable timing)
    TimestampAnomaly {
        /// Reported timestamp
        reported_time: DateTime<Utc>,
        /// Expected/consensus timestamp
        expected_time: DateTime<Utc>,
        /// Time difference in seconds
        time_difference_secs: i64,
    },
    /// Result format violation (missing required fields, invalid structure)
    FormatViolation {
        /// Description of the violation
        violation: String,
    },
    /// Multiple offense types combined
    Multiple(Vec<OffenseType>),
}

/// Context information for an offense (used for severity evaluation)
#[derive(Debug, Clone)]
pub struct OffenseContext {
    /// Provider ID that committed the offense
    pub provider_id: NodeId,
    /// Time when the offense occurred
    pub offense_time: DateTime<Utc>,
    /// Hash of the wrong result submitted
    pub wrong_result_hash: String,
    /// Hash of the correct consensus result
    pub correct_result_hash: String,
    /// Type of offense detected
    pub offense_type: OffenseType,
    /// Other providers that submitted the same wrong result (potential collusion)
    pub colluding_providers: Vec<NodeId>,
    /// Total number of providers for this task
    pub total_providers: usize,
    /// Total number of malicious providers
    pub malicious_count: usize,
    /// Resource usage information (if available)
    pub resource_usage: Option<HashMap<String, f64>>,
    /// Cost information (if available)
    pub cost_info: Option<f64>,
}

/// Penalty calculation mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PenaltyMode {
    /// Fixed amount mode (original implementation)
    FixedAmount,
    /// Proportional mode: penalty based on stake percentage (recommended)
    Proportional,
    /// Hybrid mode: combination of fixed and proportional
    Hybrid,
}

/// Enhanced slashing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashingConfig {
    /// Penalty calculation mode
    pub penalty_mode: PenaltyMode,
    /// Base penalty amount (for fixed amount mode)
    pub base_penalty: f64,
    /// Base slash ratio (for proportional mode, e.g., 0.01 = 1%)
    pub base_slash_ratio: f64,
    /// Multiplier for repeated offenses
    pub repeat_offense_multiplier: f64,
    /// Maximum penalty amount
    pub max_penalty: f64,
    /// Maximum slash ratio (e.g., 0.10 = 10%)
    pub max_slash_ratio: f64,
    /// Minimum penalty amount (prevents small stakers from escaping)
    pub min_penalty_amount: f64,
    /// Maximum penalty amount (prevents excessive single penalties)
    pub max_penalty_amount: Option<f64>,
    /// Severity-based penalty ratios
    pub severity_ratios: HashMap<OffenseSeverity, f64>,
}

impl Default for SlashingConfig {
    fn default() -> Self {
        let mut severity_ratios = HashMap::new();
        severity_ratios.insert(OffenseSeverity::Minor, 0.005);    // 0.5%
        severity_ratios.insert(OffenseSeverity::Moderate, 0.01);  // 1%
        severity_ratios.insert(OffenseSeverity::Severe, 0.05);    // 5%

        Self {
            penalty_mode: PenaltyMode::Proportional,
            base_penalty: 1.0,
            base_slash_ratio: 0.01,  // 1%
            repeat_offense_multiplier: 1.5,
            max_penalty: 10.0,
            max_slash_ratio: 0.10,   // 10%
            min_penalty_amount: 1.0,
            max_penalty_amount: Some(100.0),
            severity_ratios,
        }
    }
}

/// Track provider offenses
#[derive(Debug, Clone)]
pub struct OffenseTracker {
    /// Map from provider ID to number of offenses
    offenses: HashMap<NodeId, usize>,
    /// Map from provider ID to last offense time
    last_offense_time: HashMap<NodeId, DateTime<Utc>>,
    /// Slashing configuration
    config: SlashingConfig,
}

impl OffenseTracker {
    pub fn new(config: SlashingConfig) -> Self {
        Self {
            offenses: HashMap::new(),
            last_offense_time: HashMap::new(),
            config,
        }
    }

    /// Record an offense for a provider
    pub fn record_offense(&mut self, provider_id: NodeId) {
        *self.offenses.entry(provider_id.clone()).or_insert(0) += 1;
        self.last_offense_time.insert(provider_id, Utc::now());
    }

    /// Calculate penalty for a provider (backward compatible)
    pub fn calculate_penalty(&self, provider_id: &NodeId) -> f64 {
        self.calculate_penalty_advanced(provider_id, None, None)
    }

    /// Calculate penalty with advanced options
    pub fn calculate_penalty_advanced(
        &self,
        provider_id: &NodeId,
        stake_amount: Option<f64>,
        severity: Option<OffenseSeverity>,
    ) -> f64 {
        let offense_count = self.offenses.get(provider_id).copied().unwrap_or(0);
        if offense_count == 0 {
            return 0.0;
        }

        match self.config.penalty_mode {
            PenaltyMode::FixedAmount => {
                self.calculate_penalty_fixed(provider_id)
            }
            PenaltyMode::Proportional => {
                let stake = stake_amount.unwrap_or(0.0);
                self.calculate_penalty_proportional(provider_id, stake, severity)
            }
            PenaltyMode::Hybrid => {
                let stake = stake_amount.unwrap_or(0.0);
                let fixed = self.calculate_penalty_fixed(provider_id);
                let proportional = self.calculate_penalty_proportional(provider_id, stake, severity);
                fixed.max(proportional)
            }
        }
    }

    /// Fixed amount penalty calculation (original)
    fn calculate_penalty_fixed(&self, provider_id: &NodeId) -> f64 {
        let offense_count = self.offenses.get(provider_id).copied().unwrap_or(0);
        let penalty = self.config.base_penalty
            * self.config.repeat_offense_multiplier.powi(offense_count as i32 - 1);
        penalty.min(self.config.max_penalty)
    }

    /// Proportional penalty calculation (recommended)
    fn calculate_penalty_proportional(
        &self,
        provider_id: &NodeId,
        stake_amount: f64,
        severity: Option<OffenseSeverity>,
    ) -> f64 {
        let offense_count = self.offenses.get(provider_id).copied().unwrap_or(0);
        if stake_amount <= 0.0 {
            // Fallback to fixed amount if no stake info
            return self.calculate_penalty_fixed(provider_id);
        }

        // Select base ratio based on severity
        let base_ratio = if let Some(sev) = severity {
            *self.config.severity_ratios.get(&sev)
                .unwrap_or(&self.config.base_slash_ratio)
        } else {
            self.config.base_slash_ratio
        };

        // Calculate ratio with repeat offense multiplier
        let slash_ratio = base_ratio
            * self.config.repeat_offense_multiplier.powi(offense_count as i32 - 1);

        let final_ratio = slash_ratio.min(self.config.max_slash_ratio);
        let mut penalty = stake_amount * final_ratio;

        // Apply min/max constraints
        penalty = penalty.max(self.config.min_penalty_amount);
        if let Some(max) = self.config.max_penalty_amount {
            penalty = penalty.min(max);
        }

        // Cannot exceed stake amount
        penalty.min(stake_amount)
    }

    /// Get offense count for a provider
    pub fn get_offense_count(&self, provider_id: &NodeId) -> usize {
        self.offenses.get(provider_id).copied().unwrap_or(0)
    }

    /// Get last offense time for a provider
    pub fn get_last_offense_time(&self, provider_id: &NodeId) -> Option<DateTime<Utc>> {
        self.last_offense_time.get(provider_id).copied()
    }

    /// Get all providers with offenses
    pub fn get_offending_providers(&self) -> Vec<NodeId> {
        self.offenses.keys().cloned().collect()
    }
}

/// Severity evaluator using multi-dimensional scoring
pub struct SeverityEvaluator {
    /// Historical offense records (Provider ID -> (count, last_time))
    offense_history: HashMap<NodeId, (usize, Option<DateTime<Utc>>)>,
    /// Collusion detection window (seconds)
    collusion_detection_window: i64,
    /// Repeat offense time window (seconds)
    repeat_offense_window: i64,
}

impl SeverityEvaluator {
    pub fn new() -> Self {
        Self {
            offense_history: HashMap::new(),
            collusion_detection_window: 60,   // 60 seconds
            repeat_offense_window: 3600,      // 1 hour
        }
    }

    /// Evaluate offense severity using multi-dimensional scoring
    pub fn evaluate_severity(&mut self, context: &OffenseContext) -> OffenseSeverity {
        let mut severity_score = 0.0;

        // Dimension 1: Collusion detection (weight: 40%)
        let collusion_score = self.evaluate_collusion(context);
        severity_score += collusion_score * 0.4;

        // Dimension 2: Offense frequency (weight: 25%)
        let frequency_score = self.evaluate_frequency(context);
        severity_score += frequency_score * 0.25;

        // Dimension 3: Result consistency pattern (weight: 20%)
        let pattern_score = self.evaluate_pattern(context);
        severity_score += pattern_score * 0.2;

        // Dimension 4: Historical record (weight: 15%)
        let history_score = self.evaluate_history(context);
        severity_score += history_score * 0.15;

        // Update history
        self.update_history(context);

        // Determine severity based on score
        if severity_score >= 0.7 {
            OffenseSeverity::Severe
        } else if severity_score >= 0.4 {
            OffenseSeverity::Moderate
        } else {
            OffenseSeverity::Minor
        }
    }

    /// Dimension 1: Collusion detection
    fn evaluate_collusion(&self, context: &OffenseContext) -> f64 {
        if context.colluding_providers.len() >= 2 {
            let collusion_ratio = context.colluding_providers.len() as f64 / context.total_providers as f64;
            
            if collusion_ratio >= 0.3 {
                // Over 30% collusion, very severe
                1.0
            } else if collusion_ratio >= 0.2 {
                // 20-30% collusion, severe
                0.8
            } else {
                // Less than 20% but still collusion, moderate
                0.6
            }
        } else {
            // Single node offense, not collusion
            0.2
        }
    }

    /// Dimension 2: Offense frequency
    fn evaluate_frequency(&self, context: &OffenseContext) -> f64 {
        if let Some((count, last_time)) = self.offense_history.get(&context.provider_id) {
            if let Some(last) = last_time {
                let time_since = (context.offense_time - *last).num_seconds();
                
                if time_since < self.repeat_offense_window {
                    // Repeat offense within time window
                    if *count >= 5 {
                        1.0  // 5+ offenses, very severe
                    } else if *count >= 3 {
                        0.8  // 3-4 offenses, severe
                    } else {
                        0.6  // 2 offenses, moderate
                    }
                } else {
                    // Longer time gap, possibly accidental
                    0.3
                }
            } else {
                0.2  // First offense
            }
        } else {
            0.2  // First offense
        }
    }

    /// Dimension 3: Result consistency pattern and offense type
    fn evaluate_pattern(&self, context: &OffenseContext) -> f64 {
        // Evaluate based on offense type
        match &context.offense_type {
            OffenseType::WrongResult => {
                // Simple wrong result - check for collusion pattern
                if context.colluding_providers.len() >= 2 {
                    0.8
                } else {
                    0.3
                }
            }
            OffenseType::ResourceInflation { inflation_ratio, .. } => {
                // Resource inflation severity based on ratio
                if *inflation_ratio >= 2.0 {
                    // 2x or more inflation - severe
                    0.9
                } else if *inflation_ratio >= 1.5 {
                    // 1.5x-2x inflation - moderate to severe
                    0.7
                } else {
                    // Less than 1.5x - moderate
                    0.5
                }
            }
            OffenseType::CostFraud { cost_difference, expected_cost, .. } => {
                // Cost fraud severity based on difference ratio
                let fraud_ratio = if *expected_cost > 0.0 {
                    cost_difference.abs() / expected_cost
                } else {
                    f64::INFINITY
                };
                
                if fraud_ratio >= 0.2 {
                    // 20%+ cost fraud - severe
                    0.9
                } else if fraud_ratio >= 0.1 {
                    // 10-20% cost fraud - moderate to severe
                    0.7
                } else {
                    // Less than 10% - moderate
                    0.5
                }
            }
            OffenseType::TimestampAnomaly { time_difference_secs, .. } => {
                // Timestamp anomaly severity based on difference
                if *time_difference_secs > 3600 {
                    // More than 1 hour - severe (likely intentional)
                    0.8
                } else if *time_difference_secs > 300 {
                    // More than 5 minutes - moderate
                    0.5
                } else {
                    // Less than 5 minutes - minor (might be clock drift)
                    0.2
                }
            }
            OffenseType::FormatViolation { .. } => {
                // Format violation - usually indicates malicious intent
                0.6
            }
            OffenseType::Multiple(offenses) => {
                // Multiple offenses - more severe
                let max_severity = offenses.iter()
                    .map(|o| {
                        // Create a temporary context with this offense type
                        let mut temp_context = context.clone();
                        temp_context.offense_type = o.clone();
                        self.evaluate_pattern(&temp_context)
                    })
                    .fold(0.0, f64::max);
                
                // Multiple offenses increase severity
                (max_severity * 1.2).min(1.0)
            }
        }
    }

    /// Dimension 4: Historical record
    fn evaluate_history(&self, context: &OffenseContext) -> f64 {
        if let Some((count, _)) = self.offense_history.get(&context.provider_id) {
            if *count >= 10 {
                1.0  // 10+ offenses, habitual offender
            } else if *count >= 5 {
                0.8  // 5-9 offenses, severe
            } else if *count >= 2 {
                0.5  // 2-4 offenses, moderate
            } else {
                0.3  // 1 offense, minor
            }
        } else {
            0.2  // First offense
        }
    }

    /// Update offense history
    fn update_history(&mut self, context: &OffenseContext) {
        let entry = self.offense_history
            .entry(context.provider_id.clone())
            .or_insert((0, None));
        
        entry.0 += 1;
        entry.1 = Some(context.offense_time);
    }

    /// Get offense history for a provider
    pub fn get_offense_history(&self, provider_id: &NodeId) -> Option<(usize, Option<DateTime<Utc>>)> {
        self.offense_history.get(provider_id).copied()
    }
}

impl Default for SeverityEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// Enhanced slashing action with severity information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashingAction {
    /// Provider to be penalized
    pub provider_id: NodeId,
    /// Penalty amount
    pub penalty_amount: f64,
    /// Reason for slashing
    pub reason: String,
    /// Number of offenses (including this one)
    pub offense_count: usize,
    /// Severity of the offense
    pub severity: OffenseSeverity,
}

/// Generate slashing actions for malicious providers (backward compatible)
pub fn generate_slashing_actions(
    malicious_providers: &[NodeId],
    tracker: &mut OffenseTracker,
) -> Vec<SlashingAction> {
    generate_slashing_actions_with_severity(
        malicious_providers,
        tracker,
        &mut SeverityEvaluator::new(),
        &[],
    )
}

/// Generate slashing actions with severity evaluation
pub fn generate_slashing_actions_with_severity(
    malicious_providers: &[NodeId],
    tracker: &mut OffenseTracker,
    evaluator: &mut SeverityEvaluator,
    offense_contexts: &[OffenseContext],
) -> Vec<SlashingAction> {
    let mut actions = Vec::new();

    for (idx, provider_id) in malicious_providers.iter().enumerate() {
        // Get offense context if available
        let context = offense_contexts.get(idx);
        
        // Evaluate severity
        let severity = if let Some(ctx) = context {
            evaluator.evaluate_severity(ctx)
        } else {
            // Fallback: create minimal context
            let fallback_context = OffenseContext {
                provider_id: provider_id.clone(),
                offense_time: Utc::now(),
                wrong_result_hash: String::new(),
                correct_result_hash: String::new(),
                offense_type: OffenseType::WrongResult,
                colluding_providers: vec![],
                total_providers: malicious_providers.len(),
                malicious_count: malicious_providers.len(),
                resource_usage: None,
                cost_info: None,
            };
            evaluator.evaluate_severity(&fallback_context)
        };

        // Record offense
        tracker.record_offense(provider_id.clone());
        
        // Calculate penalty (without stake info for backward compatibility)
        let penalty = tracker.calculate_penalty(provider_id);
        let offense_count = tracker.get_offense_count(provider_id);

        let reason = format!(
            "违规严重程度: {:?}, 违规次数: {}",
            severity, offense_count
        );

        actions.push(SlashingAction {
            provider_id: provider_id.clone(),
            penalty_amount: penalty,
            reason,
            offense_count,
            severity,
        });
    }

    actions
}

/// Generate slashing actions with full context (stake amount, severity, etc.)
pub fn generate_slashing_actions_advanced(
    malicious_providers: &[NodeId],
    tracker: &mut OffenseTracker,
    evaluator: &mut SeverityEvaluator,
    offense_contexts: &[OffenseContext],
    stake_amounts: &HashMap<NodeId, f64>,
) -> Vec<SlashingAction> {
    let mut actions = Vec::new();

    for (idx, provider_id) in malicious_providers.iter().enumerate() {
        // Get offense context
        let context = offense_contexts.get(idx);
        
        // Evaluate severity
        let severity = if let Some(ctx) = context {
            evaluator.evaluate_severity(ctx)
        } else {
            OffenseSeverity::Moderate  // Default
        };

        // Record offense
        tracker.record_offense(provider_id.clone());
        
        // Get stake amount
        let stake_amount = stake_amounts.get(provider_id).copied();
        
        // Calculate penalty with full context
        let penalty = tracker.calculate_penalty_advanced(
            provider_id,
            stake_amount,
            Some(severity.clone()),
        );
        
        let offense_count = tracker.get_offense_count(provider_id);

        let reason = format!(
            "违规严重程度: {:?}, 违规次数: {}, 质押金额: {:.2}",
            severity,
            offense_count,
            stake_amount.unwrap_or(0.0)
        );

        actions.push(SlashingAction {
            provider_id: provider_id.clone(),
            penalty_amount: penalty,
            reason,
            offense_count,
            severity,
        });
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::str::FromStr;

    fn node_id(s: &str) -> NodeId {
        NodeId::from_str(s).unwrap()
    }

    #[test]
    fn test_slashing_first_offense() {
        let config = SlashingConfig::default();
        let mut tracker = OffenseTracker::new(config);
        let provider_id = node_id("provider1");

        tracker.record_offense(provider_id.clone());
        let penalty = tracker.calculate_penalty(&provider_id);

        assert_eq!(penalty, 1.0); // base_penalty (fixed mode fallback)
    }

    #[test]
    fn test_slashing_proportional() {
        let config = SlashingConfig {
            penalty_mode: PenaltyMode::Proportional,
            base_slash_ratio: 0.01,
            min_penalty_amount: 1.0,
            ..Default::default()
        };
        let mut tracker = OffenseTracker::new(config);
        let provider_id = node_id("provider1");

        tracker.record_offense(provider_id.clone());
        
        // Test with stake amount
        let penalty = tracker.calculate_penalty_advanced(
            &provider_id,
            Some(1000.0),
            Some(OffenseSeverity::Moderate),
        );
        
        assert_eq!(penalty, 10.0); // 1000 * 1% = 10
    }

    #[test]
    fn test_severity_evaluator_collusion() {
        let mut evaluator = SeverityEvaluator::new();
        
        let context = OffenseContext {
            provider_id: node_id("provider1"),
            offense_time: Utc::now(),
            wrong_result_hash: "wrong".to_string(),
            correct_result_hash: "correct".to_string(),
            offense_type: OffenseType::WrongResult,
            colluding_providers: vec![
                node_id("provider1"),
                node_id("provider2"),
                node_id("provider3"),
            ],
            total_providers: 5,
            malicious_count: 3,
            resource_usage: None,
            cost_info: None,
        };

        let severity = evaluator.evaluate_severity(&context);
        assert_eq!(severity, OffenseSeverity::Severe); // 3/5 = 60% collusion
    }

    #[test]
    fn test_severity_evaluator_single_offense() {
        let mut evaluator = SeverityEvaluator::new();
        
        let context = OffenseContext {
            provider_id: node_id("provider1"),
            offense_time: Utc::now(),
            wrong_result_hash: "wrong".to_string(),
            correct_result_hash: "correct".to_string(),
            offense_type: OffenseType::WrongResult,
            colluding_providers: vec![],
            total_providers: 5,
            malicious_count: 1,
            resource_usage: None,
            cost_info: None,
        };

        let severity = evaluator.evaluate_severity(&context);
        assert_eq!(severity, OffenseSeverity::Minor); // Single offense
    }

    #[test]
    fn test_severity_evaluator_repeat_offense() {
        let mut evaluator = SeverityEvaluator::new();
        let provider_id = node_id("provider1");
        
        // First offense
        let context1 = OffenseContext {
            provider_id: provider_id.clone(),
            offense_time: Utc::now(),
            wrong_result_hash: "wrong1".to_string(),
            correct_result_hash: "correct".to_string(),
            offense_type: OffenseType::WrongResult,
            colluding_providers: vec![],
            total_providers: 5,
            malicious_count: 1,
            resource_usage: None,
            cost_info: None,
        };
        evaluator.evaluate_severity(&context1);

        // Second offense within time window
        let context2 = OffenseContext {
            provider_id: provider_id.clone(),
            offense_time: Utc::now() + Duration::seconds(100), // Within 1 hour
            wrong_result_hash: "wrong2".to_string(),
            correct_result_hash: "correct".to_string(),
            offense_type: OffenseType::WrongResult,
            colluding_providers: vec![],
            total_providers: 5,
            malicious_count: 1,
            resource_usage: None,
            cost_info: None,
        };
        let severity = evaluator.evaluate_severity(&context2);
        
        // Should be moderate or severe depending on score
        assert!(matches!(severity, OffenseSeverity::Moderate | OffenseSeverity::Severe));
    }
}
