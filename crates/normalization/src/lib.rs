//! Shared normalization and privacy filter (AG-006): a direct port of
//! `agent/core/categories.py`, `agent/core/title_classifier.py`, and the
//! bucket-accumulation logic in `agent/core/aggregator.py`. Every platform
//! collector (Windows/Linux/macOS) uses this SAME crate — the whole point
//! of porting it once, here, rather than duplicating it per platform.
//!
//! # Versioning
//!
//! The Python source has no explicit algorithm-version marker for this
//! logic today. This port introduces one: [`ALGORITHM_VERSION`]. Bump it
//! whenever the category map, the generic-utility heuristic, or the
//! bucket-accumulation semantics change — so a future analysis of a shift
//! in category/`"other"` proportions can distinguish "the algorithm
//! changed" from "the user's actual behavior changed," which today's
//! Python MVP has no structural way to do.

mod aggregation;
mod categories;
mod title_classifier;

pub use aggregation::{BucketAccumulator, Tick, UNKNOWN_APP_LABEL};
pub use categories::{categorize, category_label, processes_for_category, UNKNOWN_CATEGORY};
pub use title_classifier::{classify_title, TitleRules};

/// See the module-level doc comment's "Versioning" section.
pub const ALGORITHM_VERSION: &str = "1.0.0";
