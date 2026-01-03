//! Verifier Service
//!
//! This module implements the verification system for redundant task execution.
//! It collects results from multiple providers and uses a simplified BFT consensus
//! algorithm to determine the correct result.

pub mod attack_tests;
pub mod consensus;
pub mod error;
pub mod result_collector;
pub mod result_comparator;
pub mod result_validator;
pub mod service;
pub mod slashing;

pub use error::{VerifierError, VerifierResult};
pub use result_collector::{ProviderResult, ResultCollector, VerificationResult};
pub use service::VerifierService;
pub use attack_tests::{run_all_attack_tests, generate_test_report, AttackTestResult};
pub use result_comparator::{CommandComparison, ResultComparator};
pub use result_validator::{ResultValidator, ValidationResult};
pub use slashing::{
    OffenseContext, OffenseSeverity, OffenseTracker, OffenseType, PenaltyMode, SeverityEvaluator,
    SlashingAction, SlashingConfig,
};

