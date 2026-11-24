//! Query helpers: FTS5 query building and fuzzy ranking of the session list.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Converts free user input into a safe FTS5 query.
///
/// Every term is quoted, so FTS operators in the input are treated as text.
/// `"quoted phrases"` match as phrases; a bare single-word term is a prefix
/// match; a bare term containing punctuation (e.g. `error_classifier.rs`)
/// becomes a phrase of its parts. Terms are ANDed.
pub fn fts_query(input: &str) -> Option<String> {
    let mut terms = Vec::new();
    for (i, chunk) in input.split('"').enumerate() {
        let quoted = i % 2 == 1;
        if quoted {
            let words = words(chunk);
            if !words.is_empty() {
                terms.push(format!("\"{}\"", words.join(" ")));
            }
            continue;
        }
        for raw in chunk.split_whitespace() {
            let parts = words(raw);
            match parts.len() {
                0 => {}
                1 => terms.push(format!("\"{}\"*", parts[0])),
                _ => terms.push(format!("\"{}\"", parts.join(" "))),
            }
        }
    }
    (!terms.is_empty()).then(|| terms.join(" "))
}

/// Splits on anything that is not alphanumeric, mirroring FTS5 `unicode61`.
fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Fuzzy ranks `haystacks` against `query`; returns indices, best first.
/// An empty query keeps the original order.
pub struct Fuzzy {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Default for Fuzzy {
    fn default() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
        }
    }
}

impl Fuzzy {
    pub fn rank<'a>(
        &mut self,
        query: &str,
        haystacks: impl Iterator<Item = &'a str>,
    ) -> Vec<usize> {
        let query = query.trim();
        if query.is_empty() {
            return haystacks.enumerate().map(|(i, _)| i).collect();
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut scored: Vec<(u32, usize)> = haystacks
            .enumerate()
            .filter_map(|(i, h)| {
                let score = pattern.score(Utf32Str::new(h, &mut self.buf), &mut self.matcher)?;
                Some((score, i))
            })
            .collect();
        // Stable on index so equal scores keep recency order.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, i)| i).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_prefix_terms_and_phrases() {
        assert_eq!(
            fts_query("retry loop").as_deref(),
            Some("\"retry\"* \"loop\"*")
        );
        assert_eq!(
            fts_query("\"connector timeout\" x").as_deref(),
            Some("\"connector timeout\" \"x\"*")
        );
        assert_eq!(
            fts_query("error_classifier.rs").as_deref(),
            Some("\"error classifier rs\"")
        );
    }

    #[test]
    fn neutralises_fts_syntax() {
        assert_eq!(
            fts_query("a OR b NOT c*").as_deref(),
            Some("\"a\"* \"OR\"* \"b\"* \"NOT\"* \"c\"*")
        );
        assert_eq!(
            fts_query("col:val (x)").as_deref(),
            Some("\"col val\" \"x\"*")
        );
        assert_eq!(fts_query("  \"\" -- ").as_deref(), None);
        assert_eq!(
            fts_query("\"unterminated phrase").as_deref(),
            Some("\"unterminated phrase\"")
        );
    }

    #[test]
    fn fuzzy_ranks_matches_and_drops_misses() {
        let hay = [
            "www/demo Fix webhook retry",
            "home statusline",
            "www/hyperswitch retry backoff",
        ];
        let mut f = Fuzzy::default();
        let r = f.rank("retry", hay.iter().copied());
        assert_eq!(r.len(), 2);
        assert!(!r.contains(&1));
        assert_eq!(f.rank("", hay.iter().copied()), vec![0, 1, 2]);
    }
}
