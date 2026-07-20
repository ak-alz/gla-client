//! Operational update-outcome reporting — deliberately, structurally
//! unable to carry any personal activity data (per the task's own
//! "operational telemetry only" / "no activity payload" constraint):
//! `UpdateTelemetryReport`'s field set is closed (no catch-all
//! map/`serde_json::Value`, no free-form string beyond version/URL
//! identifiers already public in the manifest itself) and a test below
//! asserts its serialized JSON has EXACTLY the expected key set — the
//! same "closed field set, verified by test" discipline already used
//! for privacy elsewhere in this project (e.g. `RawSignalSnapshot`
//! never carrying a window title).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use update_manifest::{Architecture, Channel, Platform};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateOutcome {
    Applied,
    RolledBack,
    DownloadFailed,
    ChecksumMismatch,
}

/// What gets reported after an update attempt — operational facts
/// about the UPDATE MECHANISM itself (which versions, which channel/
/// platform, did it succeed), never anything about how the device is
/// used. `from_version`/`to_version` are software version numbers, not
/// user data; `channel`/`platform`/`architecture` are already public
/// facts inside the (signed, non-secret) manifest this report is
/// about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTelemetryReport {
    pub from_version: semver::Version,
    pub to_version: semver::Version,
    pub channel: Channel,
    pub platform: Platform,
    pub architecture: Architecture,
    pub outcome: UpdateOutcome,
    pub occurred_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn sample() -> UpdateTelemetryReport {
        UpdateTelemetryReport {
            from_version: semver::Version::new(1, 0, 0),
            to_version: semver::Version::new(1, 1, 0),
            channel: Channel::Stable,
            platform: Platform::Linux,
            architecture: Architecture::X86_64,
            outcome: UpdateOutcome::Applied,
            occurred_at: DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    /// The single most important test in this module: proves the
    /// serialized shape carries EXACTLY the operational fields above —
    /// no accidental extra field (e.g. a device id, a category, a
    /// window/process name) could be added elsewhere in this crate and
    /// silently start flowing through this report without this test
    /// catching it.
    #[test]
    fn serialized_report_has_exactly_the_expected_field_set_no_more_no_less() {
        let json = serde_json::to_value(sample()).unwrap();
        let obj = json
            .as_object()
            .expect("report serializes as a JSON object");
        let keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = [
            "from_version",
            "to_version",
            "channel",
            "platform",
            "architecture",
            "outcome",
            "occurred_at",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn round_trips_through_json() {
        let report = sample();
        let json = serde_json::to_string(&report).unwrap();
        let parsed: UpdateTelemetryReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.from_version, report.from_version);
        assert_eq!(parsed.to_version, report.to_version);
        assert_eq!(parsed.outcome, report.outcome);
    }
}
