use crate::payload::Payload;
use chrono::{DateTime, Utc};
use thiserror::Error;

/// A single, specific way a candidate record fails to be a well-formed
/// event. Deliberately one enum per distinct failure (not a single generic
/// "invalid" variant) so a quarantined event's reason is inspectable and
/// testable, not just a boolean.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ContractViolation {
    #[error("period_end ({period_end}) is before period_start ({period_start})")]
    PeriodInverted {
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    },

    #[error("signals.active_seconds is negative: {0}")]
    NegativeActiveSeconds(f64),

    #[error("signals.active_seconds is not finite: {0}")]
    NonFiniteActiveSeconds(f64),

    #[error("signals.idle_seconds is negative: {0}")]
    NegativeIdleSeconds(f64),

    #[error("signals.idle_seconds is not finite: {0}")]
    NonFiniteIdleSeconds(f64),

    #[error("activity_segments[{index}] has ended_at before started_at")]
    SegmentInverted { index: usize },

    #[error("activity_segments[{index}] has negative duration_seconds: {duration}")]
    SegmentNegativeDuration { index: usize, duration: f64 },

    #[error("activity_segments[{index}] has non-finite duration_seconds: {duration}")]
    SegmentNonFiniteDuration { index: usize, duration: f64 },

    #[error("unexplained_gaps[{index}] has ended_at before started_at")]
    GapInverted { index: usize },

    #[error("unexplained_gaps[{index}] has negative duration_seconds: {duration}")]
    GapNegativeDuration { index: usize, duration: f64 },

    #[error("unexplained_gaps[{index}] has non-finite duration_seconds: {duration}")]
    GapNonFiniteDuration { index: usize, duration: f64 },

    #[error("empty category name at {field}[{index}]")]
    EmptyCategoryName { field: &'static str, index: usize },

    #[error("negative seconds for category {category:?} in {field}: {seconds}")]
    NegativeCategorySeconds {
        field: &'static str,
        category: String,
        seconds: f64,
    },

    #[error("non-finite seconds for category {category:?} in {field}: {seconds}")]
    NonFiniteCategorySeconds {
        field: &'static str,
        category: String,
        seconds: f64,
    },

    #[error("negative git_commits_count: {0}")]
    NegativeGitCommitsCount(i64),

    #[error("{field} is populated but consent.{consent_flag} is false")]
    MissingConsent {
        field: &'static str,
        consent_flag: &'static str,
    },
}

fn check_category_map(
    field: &'static str,
    map: &Option<std::collections::BTreeMap<String, f64>>,
    violations: &mut Vec<ContractViolation>,
) {
    let Some(map) = map else { return };
    for (index, (category, seconds)) in map.iter().enumerate() {
        if category.trim().is_empty() {
            violations.push(ContractViolation::EmptyCategoryName { field, index });
        }
        if !seconds.is_finite() {
            violations.push(ContractViolation::NonFiniteCategorySeconds {
                field,
                category: category.clone(),
                seconds: *seconds,
            });
        } else if *seconds < 0.0 {
            violations.push(ContractViolation::NegativeCategorySeconds {
                field,
                category: category.clone(),
                seconds: *seconds,
            });
        }
    }
}

/// Runs every check and returns ALL violations found (not just the first),
/// so a quarantined event's record explains everything wrong with it at
/// once — useful both for a human debugging a bad agent build and for a
/// test asserting on a specific violation among several.
pub fn validate(payload: &Payload) -> Vec<ContractViolation> {
    let mut violations = Vec::new();

    if payload.period_end < payload.period_start {
        violations.push(ContractViolation::PeriodInverted {
            period_start: payload.period_start,
            period_end: payload.period_end,
        });
    }

    // `is_finite()` is checked before the sign: NaN compares false to every
    // ordering (`NaN < 0.0` is false), so a NaN or +/-Infinity payload would
    // otherwise sail past a plain negativity check with zero violations —
    // and IEEE-754 non-finite floats serialize to JSON `null` (serde_json
    // maps them there rather than erroring), which would then fail the real
    // backend's required, non-optional `active_seconds` field far
    // downstream as an opaque 422 instead of being quarantined locally here.
    if !payload.signals.active_seconds.is_finite() {
        violations.push(ContractViolation::NonFiniteActiveSeconds(
            payload.signals.active_seconds,
        ));
    } else if payload.signals.active_seconds < 0.0 {
        violations.push(ContractViolation::NegativeActiveSeconds(
            payload.signals.active_seconds,
        ));
    }

    if let Some(idle) = payload.signals.idle_seconds {
        if !idle.is_finite() {
            violations.push(ContractViolation::NonFiniteIdleSeconds(idle));
        } else if idle < 0.0 {
            violations.push(ContractViolation::NegativeIdleSeconds(idle));
        }
    }

    if let Some(segments) = &payload.signals.activity_segments {
        for (index, segment) in segments.iter().enumerate() {
            if segment.ended_at < segment.started_at {
                violations.push(ContractViolation::SegmentInverted { index });
            }
            if !segment.duration_seconds.is_finite() {
                violations.push(ContractViolation::SegmentNonFiniteDuration {
                    index,
                    duration: segment.duration_seconds,
                });
            } else if segment.duration_seconds < 0.0 {
                violations.push(ContractViolation::SegmentNegativeDuration {
                    index,
                    duration: segment.duration_seconds,
                });
            }
        }
    }

    if let Some(gaps) = &payload.signals.unexplained_gaps {
        for (index, gap) in gaps.iter().enumerate() {
            if gap.ended_at < gap.started_at {
                violations.push(ContractViolation::GapInverted { index });
            }
            if !gap.duration_seconds.is_finite() {
                violations.push(ContractViolation::GapNonFiniteDuration {
                    index,
                    duration: gap.duration_seconds,
                });
            } else if gap.duration_seconds < 0.0 {
                violations.push(ContractViolation::GapNegativeDuration {
                    index,
                    duration: gap.duration_seconds,
                });
            }
        }
    }

    if let Some(count) = payload.signals.git_commits_count {
        if count < 0 {
            violations.push(ContractViolation::NegativeGitCommitsCount(count));
        }
    }

    check_category_map(
        "signals.active_app_category_seconds",
        &payload.signals.active_app_category_seconds,
        &mut violations,
    );
    check_category_map(
        "signals.app_seconds",
        &payload.signals.app_seconds,
        &mut violations,
    );
    check_category_map(
        "signals.other_app_seconds",
        &payload.signals.other_app_seconds,
        &mut violations,
    );

    // Consent must actually gate the data present — a populated field whose
    // consent flag is false is a contradiction (bug or privacy violation),
    // never a merely-unusual-but-valid record. This is the structural
    // mechanism `AGENT_EVENT_PARITY.md` §3 notes is currently missing (today
    // it is only a code-review convention around `title_classifier.py`).
    if payload.signals.active_app_category_seconds.is_some() && !payload.consent.active_app_category
    {
        violations.push(ContractViolation::MissingConsent {
            field: "signals.active_app_category_seconds",
            consent_flag: "active_app_category",
        });
    }
    if payload.signals.input_activity_events.is_some() && !payload.consent.input_activity_counts {
        violations.push(ContractViolation::MissingConsent {
            field: "signals.input_activity_events",
            consent_flag: "input_activity_counts",
        });
    }
    if payload.signals.idle_seconds.is_some() && !payload.consent.idle_tracking {
        violations.push(ContractViolation::MissingConsent {
            field: "signals.idle_seconds",
            consent_flag: "idle_tracking",
        });
    }
    if payload.signals.activity_segments.is_some() && !payload.consent.activity_segments {
        violations.push(ContractViolation::MissingConsent {
            field: "signals.activity_segments",
            consent_flag: "activity_segments",
        });
    }
    if payload.signals.unexplained_gaps.is_some() && !payload.consent.unexplained_gaps {
        violations.push(ContractViolation::MissingConsent {
            field: "signals.unexplained_gaps",
            consent_flag: "unexplained_gaps",
        });
    }
    if payload.signals.git_commits_count.is_some() && !payload.consent.git_activity {
        violations.push(ContractViolation::MissingConsent {
            field: "signals.git_commits_count",
            consent_flag: "git_activity",
        });
    }
    // app_seconds/other_app_seconds are double-gated in the real backend
    // (active_app_category AND app_detail) — see AGENT_EVENT_PARITY.md §1.
    if payload.signals.app_seconds.is_some()
        && !(payload.consent.active_app_category && payload.consent.app_detail)
    {
        violations.push(ContractViolation::MissingConsent {
            field: "signals.app_seconds",
            consent_flag: "active_app_category+app_detail",
        });
    }
    if payload.signals.other_app_seconds.is_some()
        && !(payload.consent.active_app_category && payload.consent.app_detail)
    {
        violations.push(ContractViolation::MissingConsent {
            field: "signals.other_app_seconds",
            consent_flag: "active_app_category+app_detail",
        });
    }

    violations
}
