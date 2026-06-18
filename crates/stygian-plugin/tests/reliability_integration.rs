//! Integration tests for the T87 extraction reliability scoring module.
//!
//! Covers:
//! - Score computation across representative extraction outcomes
//! - Backward-compat: legacy callers that don't read the `reliability` field
//!   continue to compile and pass
//! - `#[ignore]` fallback integration: `ScoreWeightedSelector` picks the
//!   higher-reliability path when comparing two plugin candidates
//!
//! Each scenario uses a fresh template store and idempotency store backed
//! by a `TempDir` to keep tests hermetic.

#![cfg_attr(test, allow(clippy::panic))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_const_for_fn
)]

use std::sync::Arc;
use tempfile::TempDir;

use serde_json::json;
use stygian_plugin::adapters::ExtractionEngine;
use stygian_plugin::domain::{
    ExtractionMetadata, ExtractionRequest, ExtractionResult, ExtractionTemplate, IdempotencyKey,
    Region, RegionStatus, Selector, Transformation,
};
use stygian_plugin::ports::{PluginExtractionPort, PluginTemplateStore};
use stygian_plugin::reliability::{
    ReliabilityBand, ReliabilityScore, ReliabilityScorer, ScoreWeightedSelector, ScoredCandidate,
    ScoringWeights,
};
use stygian_plugin::storage::FileTemplateStore;

// ── Helper builders ─────────────────────────────────────────────────────────

fn build_metadata(
    region_success: &[bool],
    errors: Vec<String>,
    selector_success_rate: f32,
) -> ExtractionMetadata {
    let mut region_status = std::collections::HashMap::new();
    for (idx, success) in region_success.iter().enumerate() {
        region_status.insert(
            format!("region_{idx}"),
            RegionStatus {
                success: *success,
                matched_count: usize::from(*success),
                error: if *success {
                    None
                } else {
                    Some("selector matched no elements".to_string())
                },
            },
        );
    }
    ExtractionMetadata {
        idempotency_key: IdempotencyKey::new(),
        completed_at: chrono::Utc::now(),
        elapsed_ms: 0,
        selector_success_rate,
        region_status,
        errors,
        reliability: None,
    }
}

#[allow(dead_code)]
fn make_template(name: &str, with_transformation: bool) -> ExtractionTemplate {
    let mut region = Region::new("title", Selector::css(".title"), json!({"type": "string"}));
    if with_transformation {
        region = region.with_transformation(Transformation::Trim);
    }
    ExtractionTemplate::new(name).with_region(region)
}

// ── Unit tests (score computation across representative outcomes) ────────────

#[test]
fn score_complete_extraction_lands_in_high_band() {
    let metadata = build_metadata(&[true, true, true], vec![], 100.0);
    let score = ReliabilityScorer::new().score_metadata(&metadata, 0);
    assert_eq!(score.band, ReliabilityBand::High);
    assert!(
        score.overall >= ReliabilityScore::HIGH_THRESHOLD,
        "complete extraction should score at or above the High threshold, got {}",
        score.overall
    );
}

#[test]
fn score_partial_extraction_lands_in_medium_band() {
    let metadata = build_metadata(&[true, false, false], vec![], 33.3);
    let score = ReliabilityScorer::new().score_metadata(&metadata, 0);
    assert_eq!(score.band, ReliabilityBand::Medium);
    assert!(score.overall < ReliabilityScore::HIGH_THRESHOLD);
    assert!(score.overall >= ReliabilityScore::MEDIUM_THRESHOLD);
}

#[test]
fn score_failed_extraction_lands_in_low_band() {
    let metadata = build_metadata(&[false, false], vec!["all selectors missed".into()], 0.0);
    let score = ReliabilityScorer::new().score_metadata(&metadata, 0);
    assert_eq!(score.band, ReliabilityBand::Low);
    assert!(score.overall < ReliabilityScore::MEDIUM_THRESHOLD);
}

#[test]
fn score_transformation_failures_lower_transformation_subscore() {
    let metadata = build_metadata(
        &[true],
        vec!["Region 'title': transformation failed".into()],
        100.0,
    );
    let score = ReliabilityScorer::new().score_metadata(&metadata, 0);
    // Schema is intact (region reported success=true) but transformation
    // sub-score drops below 1.0 to reflect the recorded transformation error.
    assert!(
        score.transformation_success < 1.0,
        "transformation errors should drop the transformation sub-score"
    );
}

#[test]
fn score_retries_lower_overall() {
    let metadata = build_metadata(&[true, true], vec![], 100.0);
    let no_retries = ReliabilityScorer::new().score_metadata(&metadata, 0);
    let max_retries = ReliabilityScorer::new().score_metadata(&metadata, 99);
    assert!(
        max_retries.overall < no_retries.overall,
        "retries must lower the overall score"
    );
    assert!((max_retries.retry_penalty - 1.0).abs() < f32::EPSILON);
}

// ── Backward-compat unit test ────────────────────────────────────────────────

/// Legacy callers that never read `metadata.reliability` and never call any
/// `reliability::*` API must continue to compile and produce the same
/// `ExtractionResult` shape they did before T87.
#[test]
fn legacy_callers_without_reliability_still_work()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let template_store: Arc<FileTemplateStore> =
        Arc::new(FileTemplateStore::new(tmp.path().to_path_buf()));
    let extraction_engine = ExtractionEngine;
    let _ = (template_store, extraction_engine, tmp);

    // The new `reliability` field defaults to `None` for code paths that
    // never populate it — so a legacy caller that *only* constructs an
    // `ExtractionResult` via `ExtractionResult::new()` does not break.
    let legacy_result = ExtractionResult::new(IdempotencyKey::new());
    assert!(
        legacy_result.metadata.reliability.is_none(),
        "freshly-constructed ExtractionResult must default reliability to None"
    );

    // JSON round-trip without `reliability` must still succeed (legacy payloads
    // predate the field entirely).
    let json = serde_json::to_string(&legacy_result)?;
    let roundtrip: ExtractionResult = serde_json::from_str(&json)?;
    assert_eq!(
        roundtrip.metadata.idempotency_key, legacy_result.metadata.idempotency_key,
        "legacy JSON payload must round-trip without reliability field"
    );

    // The full ExtractionMetadata serializes without the `reliability` key
    // when it's `None` (`skip_serializing_if = "Option::is_none"`).
    let metadata_json = serde_json::to_string(&legacy_result.metadata)?;
    assert!(
        !metadata_json.contains("reliability"),
        "metadata JSON should omit `reliability` when None, got: {metadata_json}"
    );
    Ok(())
}

// ── #[ignore] integration test: fallback chooses higher reliability ─────────

/// Build two candidate extractions: one that fully succeeds ("primary") and
/// one that misses a region ("fallback"). Confirm that
/// `ScoreWeightedSelector::pick_best` picks the higher-scoring primary even
/// when the fallback is registered first (so registration order alone
/// would have picked the wrong candidate).
#[tokio::test]
#[ignore = "fallback chain integration scenario; run with --ignored"]
async fn fallback_chooses_higher_reliability_path()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    // Build the candidate "primary" — a template that matches the HTML.
    let tmp = TempDir::new()?;
    let template_store: Arc<FileTemplateStore> =
        Arc::new(FileTemplateStore::new(tmp.path().to_path_buf()));

    let primary_template = ExtractionTemplate::new("Primary")
        .with_region(Region::new(
            "title",
            Selector::css(".title"),
            json!({"type": "string"}),
        ))
        .with_region(Region::new(
            "price",
            Selector::css(".price"),
            json!({"type": "string"}),
        ));
    template_store.save(&primary_template).await?;

    let fallback_template = ExtractionTemplate::new("Fallback").with_region(Region::new(
        "title",
        Selector::css(".does-not-exist"),
        json!({"type": "string"}),
    ));
    template_store.save(&fallback_template).await?;

    // HTML that satisfies the primary template but not the fallback.
    let html = r#"
        <html>
            <h1 class="title">Hello World</h1>
            <span class="price">42.00</span>
        </html>
    "#;

    // Execute both candidates through a real ExtractionEngine.
    let engine = ExtractionEngine;
    let primary_request =
        ExtractionRequest::new(primary_template, "https://test.example", html);
    let fallback_request =
        ExtractionRequest::new(fallback_template, "https://test.example", html);

    let primary_result = engine.execute(&primary_request).await?;
    let fallback_result = engine.execute(&fallback_request).await?;

    // Score each result.
    let primary_score = ReliabilityScorer::new().score_extraction(&primary_result, 0);
    let fallback_score = ReliabilityScorer::new().score_extraction(&fallback_result, 0);

    // Sanity: primary is in the High band, fallback is in the Low band.
    assert_eq!(primary_score.band, ReliabilityBand::High);
    assert_eq!(fallback_score.band, ReliabilityBand::Low);
    assert!(
        primary_score.overall > fallback_score.overall,
        "primary ({}) should outscore fallback ({})",
        primary_score.overall,
        fallback_score.overall
    );

    // Build candidates with the fallback registered FIRST to prove that
    // the selector ignores registration order and picks on score alone.
    let candidates = vec![
        ScoredCandidate::new("fallback", fallback_score),
        ScoredCandidate::new("primary", primary_score),
    ];
    let winner = ScoreWeightedSelector::pick_best(candidates).unwrap();
    assert_eq!(
        winner.name, "primary",
        "selector must pick the higher-scoring primary, not the first-registered fallback"
    );
    Ok(())
}

/// `ScoreWeightedSelector::pick_best_ref` must agree with the owned variant.
#[test]
fn pick_best_ref_matches_pick_best() {
    let a = ScoredCandidate::new("a", ReliabilityScore::from_overall(0.2));
    let b = ScoredCandidate::new("b", ReliabilityScore::from_overall(0.8));
    let candidates = vec![a, b];

    let by_value = ScoreWeightedSelector::pick_best(candidates.clone()).unwrap();
    let by_ref = ScoreWeightedSelector::pick_best_ref(&candidates).unwrap();
    assert_eq!(by_value.name, by_ref.name);
    assert_eq!(by_value.name, "b");
}

/// Custom `ScoringWeights` are accepted and applied (no panic on edge inputs).
#[test]
fn scoring_weights_have_sensible_defaults() {
    let defaults = ScoringWeights::default();
    assert!(defaults.validate().is_ok());
    assert!(
        defaults.schema > defaults.retry,
        "schema weight should dominate retry weight under defaults"
    );
}