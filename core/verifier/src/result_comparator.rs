//! Result comparator for detecting partial tampering
//!
//! This module provides functionality to compare results at the command level,
//! enabling detection of partial result tampering where only some commands are modified.

use ya_client_model::activity::ExeScriptCommandResult;
use std::collections::HashMap;

/// Command-level comparison result
#[derive(Debug, Clone)]
pub struct CommandComparison {
    /// Command index
    pub index: u32,
    /// Whether this command matches the consensus
    pub matches: bool,
    /// Providers that submitted matching results for this command
    pub matching_providers: Vec<usize>,
    /// Providers that submitted different results for this command
    pub mismatching_providers: Vec<usize>,
}

/// Compare results at the command level to detect partial tampering
pub struct ResultComparator;

impl ResultComparator {
    /// Compare results command by command
    ///
    /// Returns a map from command index to comparison result
    pub fn compare_by_command(
        results: &[Vec<ExeScriptCommandResult>],
    ) -> HashMap<u32, CommandComparison> {
        let mut comparisons = HashMap::new();

        if results.is_empty() {
            return comparisons;
        }

        // Find the maximum number of commands
        let max_commands = results.iter().map(|r| r.len()).max().unwrap_or(0);

        // Compare each command index
        for cmd_index in 0..max_commands {
            let mut command_results: Vec<Option<&ExeScriptCommandResult>> = Vec::new();
            
            // Collect results for this command index from all providers
            for provider_result in results {
                command_results.push(provider_result.get(cmd_index));
            }

            // Find the consensus result for this command (most common)
            let consensus_opt = Self::find_consensus_for_command(&command_results);

            // Compare each provider's result with consensus
            let mut matching = Vec::new();
            let mut mismatching = Vec::new();

            if let Some(ref consensus) = consensus_opt {
                for (provider_idx, cmd_result) in command_results.iter().enumerate() {
                    if let Some(result) = cmd_result {
                        if Self::commands_match(result, consensus) {
                            matching.push(provider_idx);
                        } else {
                            mismatching.push(provider_idx);
                        }
                    } else {
                        // Missing command is considered a mismatch
                        mismatching.push(provider_idx);
                    }
                }
            } else {
                // No consensus found, all are mismatches
                for provider_idx in 0..command_results.len() {
                    mismatching.push(provider_idx);
                }
            }

            comparisons.insert(
                cmd_index as u32,
                CommandComparison {
                    index: cmd_index as u32,
                    matches: !mismatching.is_empty(),
                    matching_providers: matching,
                    mismatching_providers: mismatching,
                },
            );
        }

        comparisons
    }

    /// Find consensus result for a single command
    fn find_consensus_for_command(
        command_results: &[Option<&ExeScriptCommandResult>],
    ) -> Option<ExeScriptCommandResult> {
        // Count occurrences of each command result
        let mut counts: HashMap<String, (usize, &ExeScriptCommandResult)> = HashMap::new();

        for result_opt in command_results.iter().flatten() {
            // Serialize command result for comparison
            if let Ok(serialized) = serde_json::to_string(result_opt) {
                counts
                    .entry(serialized)
                    .and_modify(|(count, _)| *count += 1)
                    .or_insert((1, *result_opt));
            }
        }

        // Find the most common result (consensus)
        counts
            .values()
            .max_by_key(|(count, _)| *count)
            .map(|(_, result)| (*result).clone())
    }

    /// Check if two command results match
    fn commands_match(cmd1: &ExeScriptCommandResult, cmd2: &ExeScriptCommandResult) -> bool {
        // Compare key fields
        cmd1.index == cmd2.index
            && cmd1.result == cmd2.result
            && cmd1.is_batch_finished == cmd2.is_batch_finished
            // Compare stdout/stderr (normalize None and empty)
            && Self::outputs_match(&cmd1.stdout, &cmd2.stdout)
            && Self::outputs_match(&cmd1.stderr, &cmd2.stderr)
    }

    /// Compare outputs (handling None and empty strings)
    fn outputs_match(
        out1: &Option<ya_client_model::activity::CommandOutput>,
        out2: &Option<ya_client_model::activity::CommandOutput>,
    ) -> bool {
        match (out1, out2) {
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
            (Some(a), Some(b)) => {
                // Compare by serializing both
                match (serde_json::to_string(a), serde_json::to_string(b)) {
                    (Ok(s1), Ok(s2)) => s1 == s2,
                    _ => false,
                }
            }
        }
    }

    /// Detect providers with partial tampering
    ///
    /// Returns a list of provider indices that have tampered with some commands
    pub fn detect_partial_tampering(
        results: &[Vec<ExeScriptCommandResult>],
        min_mismatch_ratio: f64,
    ) -> Vec<usize> {
        let comparisons = Self::compare_by_command(results);
        let mut tampered_providers = Vec::new();

        for provider_idx in 0..results.len() {
            let mut mismatches = 0;
            let mut total_commands = 0;

            for comparison in comparisons.values() {
                total_commands += 1;
                if comparison.mismatching_providers.contains(&provider_idx) {
                    mismatches += 1;
                }
            }

            if total_commands > 0 {
                let mismatch_ratio = mismatches as f64 / total_commands as f64;
                if mismatch_ratio >= min_mismatch_ratio {
                    tampered_providers.push(provider_idx);
                }
            }
        }

        tampered_providers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ya_client_model::activity::CommandResult;

    fn create_test_command(index: u32, result: CommandResult) -> ExeScriptCommandResult {
        ExeScriptCommandResult {
            index,
            result,
            stdout: None,
            stderr: None,
            message: None,
            is_batch_finished: index == 1,
            event_date: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_compare_by_command() {
        let results = vec![
            vec![
                create_test_command(0, CommandResult::Ok),
                create_test_command(1, CommandResult::Ok),
            ],
            vec![
                create_test_command(0, CommandResult::Ok),
                create_test_command(1, CommandResult::Error), // Tampered
            ],
            vec![
                create_test_command(0, CommandResult::Ok),
                create_test_command(1, CommandResult::Ok),
            ],
        ];

        let comparisons = ResultComparator::compare_by_command(&results);
        assert_eq!(comparisons.len(), 2);

        // Command 0: all match
        let cmd0 = comparisons.get(&0).unwrap();
        assert_eq!(cmd0.matching_providers.len(), 3);
        assert_eq!(cmd0.mismatching_providers.len(), 0);

        // Command 1: provider 1 mismatches
        let cmd1 = comparisons.get(&1).unwrap();
        assert_eq!(cmd1.matching_providers.len(), 2);
        assert_eq!(cmd1.mismatching_providers.len(), 1);
        assert!(cmd1.mismatching_providers.contains(&1));
    }

    #[test]
    fn test_detect_partial_tampering() {
        let results = vec![
            vec![
                create_test_command(0, CommandResult::Ok),
                create_test_command(1, CommandResult::Ok),
            ],
            vec![
                create_test_command(0, CommandResult::Ok),
                create_test_command(1, CommandResult::Error), // 50% tampered
            ],
            vec![
                create_test_command(0, CommandResult::Ok),
                create_test_command(1, CommandResult::Ok),
            ],
        ];

        // Detect providers with >= 30% mismatch
        let tampered = ResultComparator::detect_partial_tampering(&results, 0.3);
        assert_eq!(tampered.len(), 1);
        assert_eq!(tampered[0], 1);
    }
}

