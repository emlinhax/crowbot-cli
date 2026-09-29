//! Cloudflare challenge / Turnstile detection heuristics.
//!
//! Shared logic with the Python port (cffetch/_detect.py). Keep in sync.
//!
//! CRITICAL RULE (learned the hard way): Cloudflare injects the
//! `challenge-platform` precursor script into *real* 200 pages as well.
//! Only 403/503 responses may be classified as challenges.

/// Status codes Cloudflare uses when it is NOT serving origin content.
pub const CHALLENGE_STATUSES: [u16; 2] = [403, 503];

/// Body markers of a JS/managed challenge interstitial.
const CHALLENGE_BODY_MARKERS: &[&str] = &[
    "Just a moment",
    "_cf_chl_opt",
    "_cf_chl",
    "cdn-cgi/challenge-platform",
    "challenges.cloudflare.com",
    "cf-chl-widget",
    "Enable JavaScript and cookies to continue",
];

/// Body markers of an interactive Turnstile widget (Tier 5).
const TURNSTILE_BODY_MARKERS: &[&str] =
    &["challenges.cloudflare.com/turnstile", "cf-turnstile"];

/// Classic CF WAF "you have been blocked" markers — never a bypassable
/// challenge. (Increasingly rare: modern CF challenges instead, and
/// a test site migrated mid-testing on 2026-09-29.)
const WAF_BLOCK_MARKERS: &[&str] = &[
    "Sorry, you have been blocked",
    "you have been blocked",
    "Attention Required",
    "error code: 10",
];

/// Bound on how much of the body we scan for markers.
/// 256KB: CF's own interstitials are ~5KB, but some hosts embed the
/// Turnstile widget after long preludes; scanning too little silently
/// downgrades an interactive challenge to a rotation loop.
const SCAN_LIMIT: usize = 262_144;

fn snippet(body: &[u8]) -> String {
    let end = body.len().min(SCAN_LIMIT);
    String::from_utf8_lossy(&body[..end]).into_owned()
}

fn header<'a>(headers: &'a wreq::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Raw value of CF's `cf-mitigated` header — the single most reliable
/// signal; CF emits it only when its mitigation layer actually fired.
pub fn cf_mitigated(headers: &wreq::header::HeaderMap) -> Option<&str> {
    header(headers, "cf-mitigated")
}

/// True for CF's classic hard block page. Rarely seen anymore.
pub fn is_waf_block(body: &[u8]) -> bool {
    let s = snippet(body);
    WAF_BLOCK_MARKERS.iter().any(|m| s.contains(m))
}

/// True when the response is a Cloudflare JS/managed challenge page.
pub fn is_managed_challenge(
    status: u16,
    headers: &wreq::header::HeaderMap,
    body: &[u8],
) -> bool {
    if !CHALLENGE_STATUSES.contains(&status) {
        return false;
    }

    // Strongest signal first: CF's own "we mitigated this" header.
    if cf_mitigated(headers)
        .map(|v| v.to_ascii_lowercase().contains("challenge"))
        .unwrap_or(false)
    {
        return true;
    }

    // Header fast-path: CF only emits chlray while actually challenging.
    if header(headers, "server-timing")
        .map(|v| v.contains("chlray"))
        .unwrap_or(false)
    {
        return true;
    }

    if body.is_empty() {
        return false;
    }

    let s = snippet(body);

    // Classic WAF block pages embed the same challenge scripts; exclude
    // them first — rotating fingerprints against them is pointless.
    if WAF_BLOCK_MARKERS.iter().any(|m| s.contains(m)) {
        return false;
    }

    CHALLENGE_BODY_MARKERS.iter().any(|m| s.contains(m))
}

/// True when the page embeds an interactive Turnstile widget.
pub fn is_interactive_turnstile(body: &[u8]) -> bool {
    let s = snippet(body);
    TURNSTILE_BODY_MARKERS.iter().any(|m| s.contains(m))
}

/// Best-effort extraction of a cf-turnstile sitekey from HTML.
pub fn extract_turnstile_sitekey(body: &[u8]) -> Option<String> {
    let s = snippet(body);
    for needle in ["data-sitekey=\"", "sitekey=\""] {
        if let Some(start) = s.find(needle) {
            let rest = &s[start + needle.len()..];
            if let Some(end) = rest.find('"') {
                let key = &rest[..end];
                if key.len() >= 10 {
                    return Some(key.to_string());
                }
            }
        }
    }
    if let Some(start) = s.find("sitekey=") {
        let rest = &s[start + 8..];
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .unwrap_or(rest.len());
        let key = &rest[..end];
        if key.len() >= 10 {
            return Some(key.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(pairs: &[(&str, &str)]) -> wreq::header::HeaderMap {
        let mut h = wreq::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                wreq::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                wreq::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn status_200_never_challenge_even_with_markers() {
        // The precursor-script false positive we hit in production.
        let h = wreq::header::HeaderMap::new();
        let body = b"<html>real page <script src=\"/cdn-cgi/challenge-platform/main.js\">";
        assert!(!is_managed_challenge(200, &h, body));
    }

    #[test]
    fn cf_mitigated_header_is_authoritative() {
        let h = headers_with(&[("cf-mitigated", "challenge")]);
        assert!(is_managed_challenge(403, &h, b"<html></html>"));
    }

    #[test]
    fn classic_waf_block_is_not_a_challenge() {
        let h = wreq::header::HeaderMap::new();
        let body = b"<title>Attention Required! | Cloudflare</title>
                     <h1>Sorry, you have been blocked</h1>
                     <script src=\"/cdn-cgi/challenge-platform/x.js\"></script>";
        assert!(is_waf_block(body));
        assert!(!is_managed_challenge(403, &h, body));
    }

    #[test]
    fn chlray_server_timing_counts() {
        let h = headers_with(&[("server-timing", "chlray;desc=\"abc\"")]);
        assert!(is_managed_challenge(503, &h, b""));
    }

    #[test]
    fn sitekey_extraction() {
        let body = br#"<div class="cf-turnstile" data-sitekey="0x4AAAAAAADnPIDROrmt1Wj0"></div>"#;
        assert_eq!(
            extract_turnstile_sitekey(body),
            Some("0x4AAAAAAADnPIDROrmt1Wj0".to_string())
        );
    }

    #[test]
    fn fuzz_garbage_never_crashes_and_never_false_positives_on_200() {
        let garbage: Vec<Vec<u8>> = vec![
            vec![],
            vec![0u8; 1024],
            vec![0xff, 0xfe, 0xfd].repeat(1000),
            (0u16..=255).map(|b| b as u8).collect::<Vec<_>>().repeat(200),
            b"Just a moment _cf_chl_opt cf-turnstile challenge-platform".repeat(500),
        ];
        let empty = wreq::header::HeaderMap::new();
        for body in &garbage {
            for status in [0u16, 200, 301, 400, 403, 404, 500, 503, 999] {
                let _ = is_managed_challenge(status, &empty, body);
                let _ = is_interactive_turnstile(body);
                let _ = is_waf_block(body);
                let _ = extract_turnstile_sitekey(body);
            }
            // The golden rule: 200 with every marker present is NOT a challenge.
            assert!(!is_managed_challenge(200, &empty, body));
        }
    }

    #[test]
    fn lowercase_title_is_not_a_marker() {
        let h = wreq::header::HeaderMap::new();
        assert!(!is_managed_challenge(403, &h, b"<title>just a moment...</title>"));
    }

    #[test]
    fn sitekey_rejects_junk() {
        assert_eq!(extract_turnstile_sitekey(br#"data-sitekey="""#), None);
        assert_eq!(extract_turnstile_sitekey(br#"data-sitekey="abc""#), None);
        assert_eq!(extract_turnstile_sitekey(b"no keys here"), None);
    }
}
