use std::collections::{HashMap, HashSet};
use std::path::Path;

const MAX_CHARS: usize = 50_000;
const MAX_RELEVANT_SECTIONS: usize = 3;

/// A chunked, searchable representation of an attached document.
#[derive(Clone)]
pub struct DocContext {
    pub full_text: String,
    pub summary: String,
    pub sections: Vec<DocSection>,
}

#[derive(Clone)]
pub struct DocSection {
    pub title: String,
    pub content: String,
    /// Lowercased words for matching, computed once.
    keywords: HashSet<String>,
}

// ── Public API ──

pub fn extract_text(path: &Path) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let text = match ext.as_str() {
        "txt" | "md" => std::fs::read_to_string(path).map_err(|e| e.to_string())?,
        "pdf" => pdf_extract::extract_text(path).map_err(|e| e.to_string())?,
        "docx" => extract_docx(path)?,
        _ => return Err(format!("Unsupported file type: .{ext}")),
    };

    Ok(truncate(&text, MAX_CHARS))
}

pub fn extract_text_from_bytes(filename: &str, data: &[u8]) -> Result<String, String> {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let text = match ext.as_str() {
        "txt" | "md" => String::from_utf8(data.to_vec()).map_err(|e| e.to_string())?,
        "pdf" => {
            // pdf_extract requires a file path; write to a temp file
            let tmp = std::env::temp_dir().join(format!("lmh-{}", filename));
            std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
            let result = pdf_extract::extract_text(&tmp).map_err(|e| e.to_string());
            let _ = std::fs::remove_file(&tmp);
            result?
        }
        "docx" => extract_docx_bytes(data)?,
        _ => return Err(format!("Unsupported file type: .{ext}")),
    };

    Ok(truncate(&text, MAX_CHARS))
}

/// Split raw document text into titled sections.
pub fn chunk_into_sections(text: &str) -> Vec<DocSection> {
    let mut sections = Vec::new();
    let mut current_title = String::from("Introduction");
    let mut current_content = String::new();

    for line in text.lines() {
        if let Some(heading) = detect_heading(line) {
            if !current_content.trim().is_empty() {
                sections.push(make_section(&current_title, &current_content));
            }
            current_title = heading;
            current_content.clear();
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }
    if !current_content.trim().is_empty() {
        sections.push(make_section(&current_title, &current_content));
    }

    // If the doc had no headings at all, return one big section
    if sections.is_empty() && !text.trim().is_empty() {
        sections.push(make_section("Full Document", text));
    }

    sections
}

/// Score each section against transcript text and return the top N most relevant.
pub fn find_relevant_sections<'a>(
    sections: &'a [DocSection],
    transcript_text: &str,
) -> Vec<&'a DocSection> {
    if sections.is_empty() {
        return Vec::new();
    }

    let transcript_words = extract_keywords(transcript_text);
    if transcript_words.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(&DocSection, usize)> = sections
        .iter()
        .map(|s| {
            let overlap = s.keywords.intersection(&transcript_words).count();
            (s, overlap)
        })
        .filter(|(_, score)| *score > 0)
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored
        .into_iter()
        .take(MAX_RELEVANT_SECTIONS)
        .map(|(s, _)| s)
        .collect()
}

/// Build the prompt fragment for document context based on call type.
pub fn build_doc_prompt(doc: &DocContext, mode: DocIncludeMode, transcript_text: &str) -> String {
    match mode {
        DocIncludeMode::Full => {
            format!(
                "REFERENCE DOCUMENT:\n\
                 The meeting concerns the following document. Track which sections are discussed, \
                 what feedback is given, and what changes are proposed.\n\n{}",
                doc.full_text
            )
        }
        DocIncludeMode::Relevant => {
            let relevant = find_relevant_sections(&doc.sections, transcript_text);
            let sections_text = if relevant.is_empty() {
                String::from("[No sections matched the current discussion]")
            } else {
                relevant
                    .iter()
                    .map(|s| format!("### {}\n{}", s.title, s.content.trim()))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            format!(
                "REFERENCE DOCUMENT (summary + relevant sections):\n\
                 The meeting concerns a document. Below is its summary and the sections \
                 most relevant to the current discussion.\n\n\
                 Summary: {}\n\n\
                 Relevant sections:\n{}",
                doc.summary, sections_text
            )
        }
    }
}

pub enum DocIncludeMode {
    Full,
    Relevant,
}

// ── Internals ──

fn detect_heading(line: &str) -> Option<String> {
    let trimmed = line.trim();
    // Markdown headings: # Heading, ## Heading, etc.
    if let Some(rest) = trimmed.strip_prefix('#') {
        let rest = rest.trim_start_matches('#').trim();
        if !rest.is_empty() {
            return Some(rest.to_string());
        }
    }
    // Numbered headings: "1. Title", "1.2 Title", "Section 3: Title"
    if trimmed.len() > 3 {
        let lower = trimmed.to_lowercase();
        if lower.starts_with("section ") {
            return Some(trimmed.to_string());
        }
        // "1. Title" or "1.2. Title" patterns
        let mut chars = trimmed.chars().peekable();
        let mut saw_digit = false;
        while chars.peek().map_or(false, |c| c.is_ascii_digit() || *c == '.') {
            if chars.peek().map_or(false, |c| c.is_ascii_digit()) {
                saw_digit = true;
            }
            chars.next();
        }
        if saw_digit {
            let rest: String = chars.collect();
            let rest = rest.trim();
            // Must have text after the number and be short enough to be a heading
            if !rest.is_empty() && rest.len() < 120 && !rest.contains('.') {
                return Some(trimmed.to_string());
            }
        }
    }
    // ALL CAPS lines that are short (likely headings in plain text docs)
    if trimmed.len() > 3
        && trimmed.len() < 80
        && trimmed.chars().all(|c| c.is_uppercase() || c.is_whitespace() || c.is_ascii_punctuation())
        && trimmed.chars().any(|c| c.is_alphabetic())
    {
        return Some(trimmed.to_string());
    }
    None
}

fn make_section(title: &str, content: &str) -> DocSection {
    let keywords = extract_keywords(content);
    DocSection {
        title: title.to_string(),
        content: content.to_string(),
        keywords,
    }
}

/// Extract meaningful lowercase words, filtering out stop words.
fn extract_keywords(text: &str) -> HashSet<String> {
    static STOP_WORDS: &[&str] = &[
        "a", "an", "the", "and", "or", "but", "in", "on", "at", "to", "for",
        "of", "with", "by", "is", "it", "be", "as", "do", "no", "not", "are",
        "was", "were", "been", "has", "have", "had", "this", "that", "from",
        "they", "we", "you", "he", "she", "so", "if", "my", "our", "your",
        "its", "can", "will", "just", "about", "like", "what", "when", "how",
        "all", "would", "there", "their", "which", "one", "up", "out", "then",
        "them", "than", "into", "some", "could", "other", "also", "more",
        "very", "here", "should", "now", "way", "may", "these", "those",
        "each", "make", "over", "such", "after", "right", "too", "any",
        "same", "well", "back", "going", "think", "yeah", "okay", "know",
        "got", "get", "said", "say", "let", "thing", "things", "really",
        "want", "need", "look", "see", "come", "take", "give", "good",
    ];
    let stops: HashSet<&str> = STOP_WORDS.iter().copied().collect();

    text.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 2 && !stops.contains(w.as_str()))
        .collect()
}

fn extract_docx(path: &Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    extract_docx_bytes(&data)
}

fn extract_docx_bytes(data: &[u8]) -> Result<String, String> {
    let doc = docx_rs::read_docx(data).map_err(|e| format!("{e:?}"))?;
    let mut text = String::new();
    for child in doc.document.children {
        if let docx_rs::DocumentChild::Paragraph(p) = child {
            for c in &p.children {
                if let docx_rs::ParagraphChild::Run(run) = c {
                    for rc in &run.children {
                        if let docx_rs::RunChild::Text(t) = rc {
                            text.push_str(&t.text);
                        }
                    }
                }
            }
            text.push('\n');
        }
    }
    Ok(text)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let truncated = &s[..s.floor_char_boundary(max)];
        format!("{truncated}\n\n[Document truncated at {max} characters]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_from_bytes_txt() {
        let data = b"Hello world";
        let result = extract_text_from_bytes("notes.txt", data).unwrap();
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn extract_text_from_bytes_md() {
        let data = b"# Heading\nContent";
        let result = extract_text_from_bytes("doc.md", data).unwrap();
        assert_eq!(result, "# Heading\nContent");
    }

    #[test]
    fn extract_text_from_bytes_unsupported() {
        let result = extract_text_from_bytes("image.png", b"data");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported"));
    }

    #[test]
    fn detect_heading_markdown() {
        assert_eq!(detect_heading("# Introduction"), Some("Introduction".into()));
        assert_eq!(detect_heading("## Sub Section"), Some("Sub Section".into()));
        assert_eq!(detect_heading("###Deep"), Some("Deep".into()));
    }

    #[test]
    fn detect_heading_numbered() {
        assert_eq!(detect_heading("Section 3: Overview"), Some("Section 3: Overview".into()));
    }

    #[test]
    fn detect_heading_all_caps() {
        assert_eq!(detect_heading("EXECUTIVE SUMMARY"), Some("EXECUTIVE SUMMARY".into()));
    }

    #[test]
    fn detect_heading_normal_text() {
        assert_eq!(detect_heading("This is a regular sentence with details."), None);
    }

    #[test]
    fn detect_heading_empty() {
        assert_eq!(detect_heading(""), None);
        assert_eq!(detect_heading("#"), None);
    }

    #[test]
    fn chunk_into_sections_with_headings() {
        let doc = "# Intro\nSome intro text\n# Methods\nMethod details here\n";
        let sections = chunk_into_sections(doc);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "Intro");
        assert!(sections[0].content.contains("intro text"));
        assert_eq!(sections[1].title, "Methods");
    }

    #[test]
    fn chunk_into_sections_no_headings() {
        let doc = "Just plain text\nwith multiple lines\n";
        let sections = chunk_into_sections(doc);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Introduction");
    }

    #[test]
    fn chunk_into_sections_empty() {
        let sections = chunk_into_sections("");
        assert!(sections.is_empty());
    }

    #[test]
    fn find_relevant_sections_ranks_by_overlap() {
        let sections = vec![
            make_section("Budgets", "The budget allocation for infrastructure spending"),
            make_section("Hiring", "Recruiting engineers for the platform team"),
            make_section("Budget Review", "Review quarterly budget and infrastructure costs"),
        ];
        let relevant = find_relevant_sections(&sections, "budget infrastructure costs");
        assert!(!relevant.is_empty());
        // The section with most keyword overlap should come first
        assert!(relevant[0].title.contains("Budget"));
    }

    #[test]
    fn find_relevant_sections_empty_transcript() {
        let sections = vec![make_section("Test", "some content here")];
        let relevant = find_relevant_sections(&sections, "");
        assert!(relevant.is_empty());
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 100), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(200);
        let result = truncate(&long, 50);
        assert!(result.contains("[Document truncated at 50 characters]"));
        assert!(result.len() < 200);
    }
}
