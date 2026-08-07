//! The URL-reading counterpart to `title_classifier.rs`: the ONLY place in
//! the whole agent where address-bar TEXT is ever examined. `classify_url`
//! takes the raw text a platform collector read from the browser's address
//! bar (via OS accessibility APIs — see each collector's own module for how),
//! extracts just the host, and returns ONLY a category name. The raw text —
//! and even the extracted host — must never be returned, stored, or logged
//! by any caller; this mirrors `title_classifier.rs`'s guarantee exactly,
//! just with a stricter input (a URL can carry far more than a title ever
//! did — paths, query strings, sometimes credentials in edge cases), so the
//! one-way trip out of this module is host-only, not the full raw text.

/// Ordered list of (category, patterns) — same shape and same ordering
/// contract as `title_classifier::TitleRules` (first match wins, order is
/// semantically load-bearing). Kept as a distinct type (not a type alias)
/// even though the shape is identical: `TitleRules` and `UrlRules` are
/// configured independently by the user (see `custom_categories.py`'s two
/// separate endpoints) and a caller mixing them up by accident would be a
/// real, silent bug — distinct types make that a compile error instead.
pub type UrlRules = Vec<(String, Vec<String>)>;

/// Extracts just the host from raw address-bar text, discarding scheme,
/// path, query string, and fragment immediately — everything this module
/// classifies on is this return value, never the original string. Handles
/// the common real-world shapes address bars actually show:
///   - `https://www.youtube.com/watch?v=...` (full URL, scheme present)
///   - `www.youtube.com/watch?v=...` (Chromium elides `https://` by default)
///   - `youtube.com` (bare host, nothing else)
/// Case is NOT lowered here — `classify_url` lowercases both the host and
/// the patterns at match time, same as `classify_title` does for titles.
pub fn extract_host(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Strip a scheme if present (`scheme://`) — anything before `://` is
    // discarded outright, not just the separator, since some address bars
    // show a leading search-engine hint or similar before the real scheme.
    let after_scheme = match trimmed.find("://") {
        Some(idx) => &trimmed[idx + 3..],
        None => trimmed,
    };
    // Host ends at the first `/`, `?`, or `#` — whichever comes first.
    let end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let host = &after_scheme[..end];
    // Strip userinfo (`user:pass@host`) if present — defensive: real
    // address bars essentially never show this, but a raw accessibility
    // read should never accidentally leak credentials into a category match.
    let host = host.rsplit('@').next().unwrap_or(host);
    // Strip a trailing port (`:8080`) — not part of the host for matching
    // purposes, e.g. `localhost:3000` and `localhost` should behave the same.
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// `true` only if `host` plausibly IS a domain, not arbitrary text that
/// happened to survive `extract_host` unchanged because it contained none
/// of that function's delimiters (`://`, `/`, `?`, `#`). Real leak found
/// from a real person's own domain tally: typing a phrase directly into
/// the address bar to search (a completely normal way to search from the
/// omnibox) means the address bar's LITERAL current text, mid-keystroke,
/// is the live phrase being typed, not a URL — `extract_host` correctly
/// reads that real text (that's its job), but nothing downstream checked
/// it was domain-shaped before treating it as one, so fragments like "п",
/// "па", "прото пал или бдк" ended up stored and shown as "domains."
///
/// Deliberately NOT used inside `extract_host`/`classify_url` itself —
/// `classify_url` only ever returns a category NAME the caller already
/// defined, never the extracted host, so a garbage host there can't leak
/// anything; only `Tick::domain_host` (which stores and displays the host
/// verbatim) needs this second check. Callers apply it as a filter on
/// `extract_host`'s result, e.g. `extract_host(raw).filter(|h|
/// looks_like_a_domain(h))` — see each collector's `domain_host` gate.
///
/// Deliberately conservative: only ASCII letters/digits/hyphens/dots, at
/// least one dot (so a bare single word — "авито", "localhost" — never
/// passes; an acceptable loss for "which websites," not a correctness
/// issue), no empty/leading-or-trailing-hyphen labels. Real domains
/// always satisfy this; a typed phrase essentially never does.
pub fn looks_like_a_domain(host: &str) -> bool {
    if host.is_empty() || !host.contains('.') {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// Returns the first matching category by substring (case-insensitive) on
/// the EXTRACTED HOST — never the raw address-bar text — or `None` if there
/// are no rules, nothing to extract, or nothing matched. Substring, not
/// exact-domain matching, mirrors `classify_title`'s semantics deliberately:
/// the same simple "type a keyword" UX the user already has for title rules
/// (see `TitleRulesManager.tsx`), not a stricter public-suffix-aware
/// eTLD+1 comparison — this product's rules are user-authored keyword
/// hints, not a security boundary.
pub fn classify_url(raw_address_bar_text: Option<&str>, rules: &UrlRules) -> Option<String> {
    classify_url_with_match(raw_address_bar_text, rules).map(|(category, _keyword)| category)
}

/// Same matching as `classify_url`, but also returns WHICH keyword
/// matched — same reasoning as `title_classifier::classify_title_with_match`:
/// the keyword is the user's own rule text, not the raw address, so
/// returning it doesn't weaken the "raw address never leaves this module"
/// guarantee above. Used by `BucketAccumulator` to attribute time to
/// "which rule fired" (see `aggregation.rs`'s `rule_match_seconds`).
pub fn classify_url_with_match(raw_address_bar_text: Option<&str>, rules: &UrlRules) -> Option<(String, String)> {
    let raw = raw_address_bar_text?;
    if rules.is_empty() {
        return None;
    }
    let host = extract_host(raw)?;
    let lowered = host.to_lowercase();
    for (category, patterns) in rules {
        for pattern in patterns {
            if !pattern.is_empty() && lowered.contains(&pattern.to_lowercase()) {
                return Some((category.clone(), pattern.clone()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_host_from_full_url_with_scheme() {
        assert_eq!(
            extract_host("https://www.youtube.com/watch?v=abc123"),
            Some("www.youtube.com".to_string())
        );
    }

    #[test]
    fn extracts_host_when_scheme_is_elided_like_chromium_does() {
        assert_eq!(
            extract_host("www.youtube.com/watch?v=abc123"),
            Some("www.youtube.com".to_string())
        );
    }

    #[test]
    fn extracts_bare_host_with_nothing_else() {
        assert_eq!(extract_host("youtube.com"), Some("youtube.com".to_string()));
    }

    #[test]
    fn strips_query_string_with_no_path() {
        assert_eq!(
            extract_host("example.com?ref=123"),
            Some("example.com".to_string())
        );
    }

    #[test]
    fn strips_fragment() {
        assert_eq!(
            extract_host("example.com#section"),
            Some("example.com".to_string())
        );
    }

    #[test]
    fn strips_port() {
        assert_eq!(
            extract_host("localhost:3000/dashboard"),
            Some("localhost".to_string())
        );
    }

    #[test]
    fn strips_userinfo_defensively() {
        assert_eq!(
            extract_host("https://user:pass@example.com/path"),
            Some("example.com".to_string())
        );
    }

    #[test]
    fn empty_input_is_none() {
        assert_eq!(extract_host(""), None);
        assert_eq!(extract_host("   "), None);
    }

    #[test]
    fn looks_like_a_domain_accepts_real_domains() {
        assert!(looks_like_a_domain("chatgpt.com"));
        assert!(looks_like_a_domain("youtube.com"));
        assert!(looks_like_a_domain("tools.pixelplus.ru"));
        assert!(looks_like_a_domain("xn--80ak6aa92e.com")); // punycode IDN
    }

    #[test]
    fn looks_like_a_domain_rejects_a_real_leaked_search_query() {
        // The real bug this function exists to fix: typing a phrase
        // directly into the address bar to search it (`extract_host`
        // correctly returns it unchanged — no `://`/`/`/`?`/`#` to strip
        // on) must never be treated as a domain.
        assert!(!looks_like_a_domain("прото пал или бдк"));
        assert!(!looks_like_a_domain("п"));
        assert!(!looks_like_a_domain("па"));
    }

    #[test]
    fn looks_like_a_domain_rejects_a_bare_word_without_a_dot() {
        assert!(!looks_like_a_domain("авито"));
        assert!(!looks_like_a_domain("localhost"));
    }

    #[test]
    fn looks_like_a_domain_rejects_empty_and_malformed_labels() {
        assert!(!looks_like_a_domain(""));
        assert!(!looks_like_a_domain("."));
        assert!(!looks_like_a_domain("example."));
        assert!(!looks_like_a_domain(".example.com"));
        assert!(!looks_like_a_domain("-example.com"));
        assert!(!looks_like_a_domain("example-.com"));
    }

    fn rules() -> UrlRules {
        vec![("rest".to_string(), vec!["youtube".to_string()])]
    }

    #[test]
    fn classifies_by_host_substring_case_insensitive() {
        assert_eq!(
            classify_url(Some("https://WWW.YouTube.com/watch?v=1"), &rules()),
            Some("rest".to_string())
        );
    }

    #[test]
    fn does_not_match_on_path_or_query_text_outside_the_host() {
        // "youtube" appears only in the query string here, never in the
        // host itself — must NOT match, unlike a naive whole-string search.
        let rules = vec![("rest".to_string(), vec!["youtube".to_string()])];
        assert_eq!(
            classify_url(Some("https://example.com/search?q=youtube"), &rules),
            None
        );
    }

    #[test]
    fn no_rules_means_never_classify() {
        assert_eq!(classify_url(Some("youtube.com"), &Vec::new()), None);
    }

    #[test]
    fn none_input_is_none() {
        assert_eq!(classify_url(None, &rules()), None);
    }

    #[test]
    fn first_matching_category_wins_when_rules_overlap() {
        let rules = vec![
            ("work".to_string(), vec!["docs".to_string()]),
            ("rest".to_string(), vec!["google".to_string()]),
        ];
        assert_eq!(
            classify_url(Some("https://docs.google.com/document/1"), &rules),
            Some("work".to_string())
        );
    }
}
