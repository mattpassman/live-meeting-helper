use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Correction {
    pub original: String,
    pub corrected: String,
    pub created_at: u64,
}

/// Extract word substitutions by comparing original AI text to user-edited text.
/// Uses a simple word-level alignment: walk both word lists, skip matching words,
/// and when a mismatch is found, treat single-word differences as corrections.
pub fn extract_corrections(original: &str, edited: &str) -> Vec<Correction> {
    let orig_words: Vec<&str> = original.split_whitespace().collect();
    let edit_words: Vec<&str> = edited.split_whitespace().collect();

    if orig_words.is_empty() || edit_words.is_empty() {
        return Vec::new();
    }

    let now = chrono::Utc::now().timestamp_millis() as u64;
    let mut corrections = Vec::new();
    let mut oi = 0;
    let mut ei = 0;

    while oi < orig_words.len() && ei < edit_words.len() {
        let o_clean = orig_words[oi].trim_matches(|c: char| !c.is_alphanumeric());
        let e_clean = edit_words[ei].trim_matches(|c: char| !c.is_alphanumeric());

        if o_clean == e_clean {
            oi += 1;
            ei += 1;
            continue;
        }

        // Look ahead in edited to see if orig word appears soon (user inserted words)
        let ahead_e = edit_words[ei..].iter().take(4).position(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric()) == o_clean
        });
        // Look ahead in original to see if edited word appears soon (user deleted words)
        let ahead_o = orig_words[oi..].iter().take(4).position(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric()) == e_clean
        });

        match (ahead_e, ahead_o) {
            // Edited word found ahead in original → user deleted words, skip original
            (None, Some(skip)) if skip > 0 => { oi += skip; }
            // Original word found ahead in edited → user inserted words, skip edited
            (Some(skip), None) if skip > 0 => { ei += skip; }
            // Both not found ahead or both at position 1 → likely a substitution
            _ => {
                if o_clean.len() >= 2 && e_clean.len() >= 2 && o_clean != e_clean {
                    corrections.push(Correction {
                        original: o_clean.to_string(),
                        corrected: e_clean.to_string(),
                        created_at: now,
                    });
                }
                oi += 1;
                ei += 1;
            }
        }
    }

    corrections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_correction() {
        let cs = extract_corrections("Jon will handle the review", "John will handle the review");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].original, "Jon");
        assert_eq!(cs[0].corrected, "John");
    }

    #[test]
    fn no_change() {
        let cs = extract_corrections("same text here", "same text here");
        assert!(cs.is_empty());
    }

    #[test]
    fn different_lengths_no_corrections() {
        // Pure insertion — no substitution to extract
        let cs = extract_corrections("short text", "short extra text");
        assert!(cs.is_empty());
    }

    #[test]
    fn punctuation_only_ignored() {
        let cs = extract_corrections("hello, world", "hello world");
        assert!(cs.is_empty());
    }

    #[test]
    fn substitution_with_insertion() {
        let cs = extract_corrections(
            "Jon will handle the review",
            "John will also handle the review",
        );
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].original, "Jon");
        assert_eq!(cs[0].corrected, "John");
    }

    #[test]
    fn multiple_substitutions() {
        let cs = extract_corrections(
            "Jon from Atlantis presented the results",
            "John from Atlas presented the results",
        );
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].original, "Jon");
        assert_eq!(cs[0].corrected, "John");
        assert_eq!(cs[1].original, "Atlantis");
        assert_eq!(cs[1].corrected, "Atlas");
    }
}
