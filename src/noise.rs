//! Noise-heading post-pass: drop TOC roman numerals, single-letter fragments,
//! pure-digit/date page-number footers, and repeating running headers.

use std::collections::HashMap;

use super::banner::normalize_banner;

// ---- noise-heading post-pass ------------------------------------------------

/// Maximum bytes of prose between a heading and the next heading (or EOF)
/// for the heading to be eligible for "empty section" dropping.
const NOISE_HEADING_EMPTY_PROSE_BYTES: usize = 200;
/// Roman-numeral headings get a looser threshold because they're almost
/// always TOC page-number markers (`vi`, `xii`) whose section body is a
/// list of short numeric references like `3 - 4`, not real prose.
const NOISE_HEADING_ROMAN_MAX_PROSE_BYTES: usize = 1000;
/// A heading title that — after digit-run normalization — recurs more than
/// this many times across the document is treated as a running header /
/// footer and all occurrences are dropped.
const REPETITION_THRESHOLD: usize = 4;

/// Strip heading lines that look like TOC roman numerals, single-letter
/// fragments, or pure-digit/date page-number footers. When such a heading
/// is dropped, its inner text is preserved as a plain paragraph so any
/// stray sentence-like content folds back into the previous section.
///
/// Rules (heading title trimmed):
///   * single ASCII letter `[a-zA-Z]` AND following section < 200 prose bytes
///   * roman-numeral-only `[ivxlcdmIVXLCDM]+` AND following section < 200 bytes
///   * pure digit/date — only digits, dots, dashes, slashes, spaces — at
///     least one digit, no letters — always dropped regardless of section
///     length, since these are page-number footers misclassified as headings.
pub(super) fn strip_noise_headings(md: &str) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    if lines.is_empty() {
        return String::new();
    }

    // Pre-pass: locate every heading line and the index of the next heading.
    let heading_re = regex::Regex::new(r"^(#+)\s+(.+?)\s*$").unwrap();
    let heading_idx: Vec<(usize, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            heading_re
                .captures(l)
                .map(|caps| (i, caps.get(2).unwrap().as_str().trim().to_string()))
        })
        .collect();

    let mut drop_decision: HashMap<usize, NoiseDecision> = HashMap::new();

    // Repetition pass: any heading title whose digit-run-normalized key
    // recurs more than REPETITION_THRESHOLD times in the document is a
    // running header / footer — drop every occurrence.
    let mut repetition_counts: HashMap<String, usize> = HashMap::new();
    for (_, title) in &heading_idx {
        let key = normalize_banner(title);
        if !key.is_empty() {
            *repetition_counts.entry(key).or_insert(0) += 1;
        }
    }
    for (line_i, title) in &heading_idx {
        let key = normalize_banner(title);
        if !key.is_empty()
            && repetition_counts.get(&key).copied().unwrap_or(0) > REPETITION_THRESHOLD
        {
            drop_decision.insert(
                *line_i,
                NoiseDecision {
                    title: title.clone(),
                },
            );
        }
    }

    // For each non-marked heading, classify by title pattern + section length.
    for (hi, &(line_i, ref title)) in heading_idx.iter().enumerate() {
        if drop_decision.contains_key(&line_i) {
            continue;
        }
        let next_line_i = heading_idx
            .get(hi + 1)
            .map(|(j, _)| *j)
            .unwrap_or(lines.len());
        // Prose bytes = trimmed length of non-heading lines in (line_i, next_line_i).
        let prose_bytes: usize = lines[line_i + 1..next_line_i]
            .iter()
            .map(|l| l.trim().len())
            .sum();

        if let Some(decision) = classify_noise_heading(title, prose_bytes) {
            drop_decision.insert(line_i, decision);
        }
    }

    if drop_decision.is_empty() {
        return md.to_string();
    }

    let mut out = String::with_capacity(md.len());
    for (i, line) in lines.iter().enumerate() {
        if let Some(decision) = drop_decision.get(&i) {
            // Replace heading with its inner text as a plain paragraph so
            // any sentence-like content remains visible.
            if !decision.title.is_empty() {
                out.push_str(&decision.title);
            }
        } else {
            out.push_str(line);
        }
        if i + 1 < lines.len() {
            out.push('\n');
        }
    }
    out
}

struct NoiseDecision {
    title: String,
}

fn classify_noise_heading(title: &str, prose_bytes: usize) -> Option<NoiseDecision> {
    let t = title.trim();
    if t.is_empty() {
        return None;
    }

    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let has_letter = t.chars().any(|c| c.is_alphabetic());

    // Pure punctuation / whitespace: no letters, no digits. These come from
    // PDF rendering artifacts (a stray dash or dot that the font classifier
    // promoted to a heading) and always slugify to nothing, producing
    // sibling-index fallback pages.
    if !has_letter && !has_digit {
        return Some(NoiseDecision {
            title: t.to_string(),
        });
    }

    // Pure digit / date pattern: only digits, dots, dashes, slashes, spaces.
    // At least one digit, no ASCII letters. Always drop (these are page-number
    // footers misclassified as headings).
    let only_date_chars = t
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '/' | ' '));
    if only_date_chars && has_digit && !has_letter {
        return Some(NoiseDecision {
            title: t.to_string(),
        });
    }

    // Single ASCII letter: only drop when the following section is short.
    if t.chars().count() == 1
        && t.chars().next().unwrap().is_ascii_alphabetic()
        && prose_bytes < NOISE_HEADING_EMPTY_PROSE_BYTES
    {
        return Some(NoiseDecision {
            title: t.to_string(),
        });
    }

    // Roman-numeral-only: drop when the following section is short. The
    // threshold here is looser than for letters because TOC pages headed by
    // a roman page-number have substantial body (a list of digit references)
    // that we still want to fold away.
    if !t.is_empty()
        && t.chars().all(|c| {
            matches!(
                c,
                'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm' | 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M'
            )
        })
        && prose_bytes < NOISE_HEADING_ROMAN_MAX_PROSE_BYTES
    {
        return Some(NoiseDecision {
            title: t.to_string(),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roman_numeral_heading_with_empty_section_dropped() {
        let md = "## i\nZÁMĚRNĚ NEPOUŽITO\n\n## Real heading\nBody text.";
        let out = strip_noise_headings(md);
        // The "## i" heading should be replaced with plain text "i".
        assert!(
            !out.contains("## i\n"),
            "expected '## i' heading dropped, got: {out}"
        );
        // The inner text 'i' should remain as a plain line.
        assert!(out.contains("i\nZÁMĚRNĚ NEPOUŽITO"), "got: {out}");
        // Real heading must remain.
        assert!(out.contains("## Real heading"), "got: {out}");
    }

    #[test]
    fn roman_numeral_heading_with_real_prose_kept() {
        // 1500 bytes of prose under the heading — above the 1000-byte cutoff
        // we use for roman-numeral headings.
        let prose = "x".repeat(1500);
        let md = format!("## iv\n{}\n", prose);
        let out = strip_noise_headings(&md);
        assert!(
            out.starts_with("## iv\n"),
            "expected '## iv' heading kept (section is long), got: {out}"
        );
    }

    #[test]
    fn running_header_dropped_by_repetition() {
        // 10 identical `## PŘEDPIS L14` headings each followed by some prose.
        // All should be folded — the inner title becomes a plain paragraph.
        let mut md = String::new();
        for i in 0..10 {
            md.push_str(&format!(
                "## PŘEDPIS L14\nChapter body {i} text content.\n\n"
            ));
        }
        let out = strip_noise_headings(&md);
        assert!(
            !out.contains("## PŘEDPIS L14"),
            "all '## PŘEDPIS L14' headings should be dropped, got: {out}"
        );
        // Inner title preserved as plain line.
        assert!(out.contains("PŘEDPIS L14\n"), "got: {out}");
        // Each body's prose still there.
        for i in 0..10 {
            assert!(
                out.contains(&format!("Chapter body {i}")),
                "lost body {i}: {out}"
            );
        }
    }

    #[test]
    fn title_with_varying_digits_collapses() {
        // 6 footers `Dopl. 1` .. `Dopl. 6`. After digit-run normalization they
        // all hash to `Dopl . \d`, so the repetition pass treats them as one
        // group of 6 (> threshold) and drops every one.
        let mut md = String::new();
        for i in 1..=6 {
            md.push_str(&format!("## Dopl. {i}\nstub {i}\n\n"));
        }
        let out = strip_noise_headings(&md);
        for i in 1..=6 {
            assert!(
                !out.contains(&format!("## Dopl. {i}\n")),
                "expected '## Dopl. {i}' dropped, got: {out}"
            );
        }
    }

    #[test]
    fn non_repeated_heading_kept() {
        // Six distinct headings, each appearing once. None should hit the
        // repetition threshold; all should be kept (no other noise rules fire).
        let md = "## Alpha\nbody.\n\n## Beta\nbody.\n\n## Gamma\nbody.\n\n## Delta\nbody.\n\n## Epsilon\nbody.\n\n## Zeta\nbody.";
        let out = strip_noise_headings(md);
        for name in ["Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta"] {
            assert!(
                out.contains(&format!("## {name}")),
                "expected '## {name}' kept, got: {out}"
            );
        }
    }

    #[test]
    fn punctuation_only_heading_dropped() {
        // Single dash heading — no letters, no digits → drop unconditionally.
        let md = "## -\n\n## Real\nbody.";
        let out = strip_noise_headings(md);
        assert!(
            !out.contains("## -\n"),
            "expected '## -' dropped, got: {out}"
        );
        // Em-dash + dash combo, also pure punctuation.
        let md = "## – -\n\n## Real\nbody.";
        let out = strip_noise_headings(md);
        assert!(
            !out.contains("## – -"),
            "expected '## – -' dropped, got: {out}"
        );
    }

    #[test]
    fn roman_numeral_with_toc_body_dropped() {
        // `## vi` followed by ~600 bytes of short numeric TOC entries —
        // below the 1000-byte roman threshold → drop.
        let mut body = String::new();
        for chapter in 3..=5 {
            for sub in 1..=40 {
                body.push_str(&format!("{chapter} - {sub}\n"));
            }
        }
        let md = format!("## vi\n{}", body);
        let out = strip_noise_headings(&md);
        assert!(
            !out.starts_with("## vi"),
            "expected '## vi' dropped (TOC body), got first 80 chars: {}",
            &out[..out.len().min(80)]
        );
        assert!(
            out.starts_with("vi\n"),
            "got: {}",
            &out[..out.len().min(80)]
        );
    }

    #[test]
    fn date_heading_always_dropped() {
        // Even with a long body the date heading should be dropped — it's a
        // page-number footer, not a content boundary.
        let prose = "x".repeat(500);
        let md = format!("## 25 . 12 . 2025\n{}\n", prose);
        let out = strip_noise_headings(&md);
        assert!(
            !out.starts_with("## 25"),
            "expected date heading dropped, got: {out}"
        );
        // Inner text preserved.
        assert!(out.starts_with("25 . 12 . 2025\n"), "got: {out}");
    }

    #[test]
    fn single_letter_heading_with_empty_section_dropped() {
        let md = "## c\n\n## Next\nBody.";
        let out = strip_noise_headings(md);
        assert!(
            !out.contains("## c\n"),
            "expected '## c' heading dropped, got: {out}"
        );
        // Inner text 'c' preserved as plain line.
        assert!(out.starts_with("c\n"), "got: {out}");
    }
}
