//! Validates short agent links in message prose; it never resolves recipients.
use std::sync::OnceLock;

use regex::Regex;
use url::Url;

use crate::{Error, Result};

pub(super) fn validate(text: &str) -> Result<()> {
    static MARKERS: OnceLock<Regex> = OnceLock::new();
    static LINKS: OnceLock<Regex> = OnceLock::new();
    let markers = MARKERS.get_or_init(|| Regex::new(r"`+|~{3,}").unwrap());
    let links = LINKS.get_or_init(|| {
        let space = r"\t\n\x0B\x0C\r \x{00A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";
        Regex::new(&format!(r"\]\([{space}]*<?([^{space})>]+)")).unwrap()
    });
    let mut prose = String::new();
    let mut cursor = 0;
    let mut search_from = 0;
    while let Some(marker) = markers.find_at(text, search_from) {
        let suffix = &text[marker.start()..];
        let fence_end = code_end(suffix, marker.len(), markers, 3);
        let end = fence_end.or_else(|| {
            (marker.as_str().starts_with('`'))
                .then(|| {
                    code_end(
                        suffix.split('\n').next().unwrap_or_default(),
                        marker.len(),
                        markers,
                        1,
                    )
                })
                .flatten()
        });
        if let Some(end) = end {
            prose.push_str(&text[cursor..marker.start()]);
            prose.push(' ');
            cursor = marker.start() + end;
            search_from = cursor;
        } else {
            search_from = marker.end();
        }
    }
    prose.push_str(&text[cursor..]);
    for capture in links.captures_iter(&prose) {
        let target = &capture[1];
        if target.starts_with("bc-") && !valid_agent_link(target) {
            return Err(Error::Protocol(format!("Invalid agent link: {target}. Use a known agent ID, optionally followed by #changes or #desktop, or name the agent without a link.")));
        }
    }
    Ok(())
}

// Match the native greedy delimiter / lazy body without a backtracking engine.
// A shorter closing run can shrink the opening capture; a single run can also
// contain both delimiters. Scan again after the consumed closing delimiter.
fn code_end(text: &str, opening: usize, markers: &Regex, minimum: usize) -> Option<usize> {
    if opening < minimum {
        return None;
    }
    let byte = text.as_bytes()[0];
    let later = markers
        .find_iter(&text[opening..])
        .filter(|marker| marker.as_str().as_bytes()[0] == byte)
        .map(|marker| marker.len())
        .max()
        .unwrap_or(0);
    let length = (opening / 2).max(opening.min(later));
    if length < minimum {
        return None;
    }
    let delimiter = &text[..length];
    text[length..]
        .find(delimiter)
        .map(|close| length + close + length)
}

fn valid_agent_link(target: &str) -> bool {
    let base = Url::parse("https://agent-link.invalid/").unwrap();
    let Ok(url) = base.join(target) else {
        return false;
    };
    if url.origin() != base.origin() || url.query().is_some_and(|query| !query.is_empty()) {
        return false;
    }
    let parts = url
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if !(1..=2).contains(&parts.len()) || url.path() != format!("/{}", parts.join("/")) {
        return false;
    }
    let Some(id) = parts[0].strip_prefix("bc-") else {
        return false;
    };
    let Ok(uuid) = uuid::Uuid::parse_str(id) else {
        return false;
    };
    if !uuid.hyphenated().to_string().eq_ignore_ascii_case(id) {
        return false;
    }
    let fragment = url.fragment().unwrap_or_default();
    if parts.len() == 2 {
        return fragment.is_empty() && matches!(parts[1], "changes" | "desktop");
    }
    // Native links with a malformed file selector still identify the agent;
    // file interpretation belongs to the client, not message delivery.
    fragment.is_empty()
        || matches!(fragment, "changes" | "desktop")
        || fragment
            .split('&')
            .any(|part| part.split('=').next() == Some("file"))
}
