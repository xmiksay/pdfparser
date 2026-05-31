/// A font signature captures the visual characteristics of a text run that determine
/// whether it is body text, a heading, or inline formatting.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FontSignature {
    /// Font size rounded to the nearest 0.5pt (stored as half-points to stay integer).
    /// E.g. 12.0pt -> 24, 10.5pt -> 21.
    pub size_bucket: u32,
    pub is_bold: bool,
    pub is_italic: bool,
}

impl FontSignature {
    /// Create a signature from a raw font size in points.
    /// Rounds to nearest 0.5pt and stores as an integer (half-points).
    pub fn new(size_pts: f32, is_bold: bool, is_italic: bool) -> Self {
        // Round to nearest 0.5: multiply by 2, round, keep as u32.
        let size_bucket = (size_pts * 2.0).round().max(0.0) as u32;
        Self {
            size_bucket,
            is_bold,
            is_italic,
        }
    }
}

/// Classifies font signatures into heading levels based on a histogram of character counts.
///
/// The signature with the largest total character weight is treated as body text.
/// Signatures with `size_bucket` strictly larger than body are candidate headings,
/// filtered by char-count to drop one-off oversized text (e.g. cover-page numbers),
/// sorted descending by `size_bucket`, deduplicated, and capped at 4 levels (H1..H4).
pub struct HeadingClassifier {
    pub body: FontSignature,
    /// Index 0 = H1, index 1 = H2, etc. Length <= 4.
    pub levels: Vec<FontSignature>,
}

/// Maximum heading depth assigned by the classifier. Anything deeper would
/// usually be cover-page noise (one-off oversized text) rather than real
/// document structure.
pub const MAX_HEADING_DEPTH: usize = 4;
/// Floor on the per-size character count required for a font size to be
/// treated as a heading. Prevents single-page artifacts (e.g. a publication
/// number rendered slightly larger than body) from inflating heading levels.
const MIN_HEADING_CHARS_FLOOR: usize = 50;
/// Relative threshold: a candidate heading size must contribute at least
/// `body_chars / HEADING_CHARS_BODY_RATIO` characters across the document.
const HEADING_CHARS_BODY_RATIO: usize = 5000;
/// Diversity audit: a font level's top-K distinct trimmed strings are
/// compared against its total char budget. If those few strings hog more
/// than `HEADING_DIVERSITY_MAX_RATIO` of the budget, the level is dropped —
/// it's almost certainly a running header that slipped past banner detection
/// rather than a real heading family.
const HEADING_DIVERSITY_TOP_K: usize = 5;
const HEADING_DIVERSITY_MAX_RATIO: f32 = 0.80;

impl HeadingClassifier {
    /// Build a classifier from an iterator of `(signature, text)` pairs.
    ///
    /// The char-count for each sample is derived from `text.chars().count()`.
    /// The classifier also performs a diversity audit per candidate level —
    /// if the top-5 distinct trimmed strings dominate the level's char budget
    /// (> 80%), the level is dropped as a likely running header.
    ///
    /// Panics if the iterator is empty (there must be at least one text sample).
    pub fn build<'a>(samples: impl Iterator<Item = (FontSignature, &'a str)>) -> Self {
        use std::collections::HashMap;

        // Per-signature totals (for body detection) and per-size-bucket text
        // tallies (for the diversity audit, applied later by size_bucket).
        let mut histogram: HashMap<FontSignature, usize> = HashMap::new();
        let mut per_size_text_counts: HashMap<u32, HashMap<String, usize>> = HashMap::new();
        let mut per_size_total: HashMap<u32, usize> = HashMap::new();

        for (sig, text) in samples {
            let count = text.chars().count();
            *histogram.entry(sig.clone()).or_insert(0) += count;
            let bucket = sig.size_bucket;
            *per_size_total.entry(bucket).or_insert(0) += count;
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                *per_size_text_counts
                    .entry(bucket)
                    .or_default()
                    .entry(trimmed)
                    .or_insert(0) += count;
            }
        }

        assert!(
            !histogram.is_empty(),
            "HeadingClassifier requires at least one font sample"
        );

        // Body text is the signature with the highest total character count.
        let body = histogram
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(sig, _)| sig.clone())
            .unwrap();

        let body_chars = *histogram.get(&body).unwrap_or(&0);
        let min_heading_chars = std::cmp::max(
            MIN_HEADING_CHARS_FLOOR,
            body_chars / HEADING_CHARS_BODY_RATIO,
        );

        // Aggregate char counts by size_bucket so the threshold is applied to
        // the total weight of a size (across bold/italic variants).
        let mut by_size: HashMap<u32, (FontSignature, usize)> = HashMap::new();
        for (sig, count) in &histogram {
            if sig.size_bucket <= body.size_bucket {
                continue;
            }
            let entry = by_size
                .entry(sig.size_bucket)
                .or_insert_with(|| (sig.clone(), 0));
            entry.1 += *count;
        }

        let mut candidates: Vec<(FontSignature, usize)> = by_size
            .into_values()
            .filter(|(_, c)| *c >= min_heading_chars)
            .filter(|(sig, _)| {
                // Diversity audit: drop levels where a few repeated strings
                // dominate the budget.
                let total = *per_size_total.get(&sig.size_bucket).unwrap_or(&0);
                if total == 0 {
                    return true;
                }
                let Some(text_counts) = per_size_text_counts.get(&sig.size_bucket) else {
                    return true;
                };
                let mut counts: Vec<usize> = text_counts.values().copied().collect();
                counts.sort_unstable_by(|a, b| b.cmp(a));
                let top_k_sum: usize = counts.iter().take(HEADING_DIVERSITY_TOP_K).sum();
                let ratio = top_k_sum as f32 / total as f32;
                ratio <= HEADING_DIVERSITY_MAX_RATIO
            })
            .collect();
        candidates.sort_by_key(|c| std::cmp::Reverse(c.0.size_bucket));
        candidates.truncate(MAX_HEADING_DEPTH);

        Self {
            body,
            levels: candidates.into_iter().map(|(s, _)| s).collect(),
        }
    }

    /// Classify a font signature.
    ///
    /// Returns `Some(1..=6)` for heading levels, `None` for body text or below.
    pub fn classify(&self, sig: &FontSignature) -> Option<u8> {
        if sig.size_bucket <= self.body.size_bucket {
            return None;
        }
        self.levels
            .iter()
            .position(|level| level.size_bucket == sig.size_bucket)
            .map(|idx| (idx as u8) + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build many synthetic distinct strings of the given length so
    /// that the diversity audit treats the level as varied content.
    fn diverse_samples(
        sig: FontSignature,
        total_chars: usize,
        str_len: usize,
    ) -> Vec<(FontSignature, String)> {
        if str_len == 0 {
            return vec![(sig, String::new())];
        }
        let n = total_chars.div_ceil(str_len);
        (0..n)
            .map(|i| {
                // Stamp each sample with its index so trimmed strings differ.
                let base = format!("h{:08}", i);
                let mut s = base.clone();
                if s.len() < str_len {
                    s.push_str(&"x".repeat(str_len - s.len()));
                } else {
                    s.truncate(str_len);
                }
                (sig.clone(), s)
            })
            .collect()
    }

    fn samples_iter(
        samples: &[(FontSignature, String)],
    ) -> impl Iterator<Item = (FontSignature, &str)> {
        samples.iter().map(|(s, t)| (s.clone(), t.as_str()))
    }

    #[test]
    fn only_body_text() {
        let body_sig = FontSignature::new(12.0, false, false);
        let body_samples = diverse_samples(body_sig.clone(), 5000, 50);
        let classifier = HeadingClassifier::build(samples_iter(&body_samples));

        assert_eq!(classifier.body, body_sig);
        assert!(classifier.levels.is_empty());
        assert_eq!(classifier.classify(&body_sig), None);
    }

    #[test]
    fn body_plus_two_heading_sizes() {
        let body = FontSignature::new(12.0, false, false);
        let h1 = FontSignature::new(24.0, true, false);
        let h2 = FontSignature::new(18.0, true, false);

        // Use short distinct strings so the diversity audit sees > 5 unique
        // titles per heading level (otherwise the top-5 would dominate the
        // budget and the level would be dropped).
        let mut samples = diverse_samples(body.clone(), 5000, 50);
        samples.extend(diverse_samples(h1.clone(), 200, 10));
        samples.extend(diverse_samples(h2.clone(), 300, 10));

        let classifier = HeadingClassifier::build(samples_iter(&samples));

        assert_eq!(classifier.body, body);
        assert_eq!(classifier.levels.len(), 2);
        assert_eq!(classifier.classify(&h1), Some(1));
        assert_eq!(classifier.classify(&h2), Some(2));
        assert_eq!(classifier.classify(&body), None);

        // Bold-but-body-sized should not be a heading.
        let bold_body = FontSignature::new(12.0, true, false);
        assert_eq!(classifier.classify(&bold_body), None);
    }

    #[test]
    fn more_than_four_heading_sizes_capped() {
        let body = FontSignature::new(10.0, false, false);
        // 6 distinct heading sizes above body, all well above the rare-size floor.
        let headings: Vec<FontSignature> = (1..=6)
            .map(|i| FontSignature::new(10.0 + i as f32 * 2.0, false, false))
            .collect();

        let mut samples = diverse_samples(body.clone(), 10_000, 50);
        for (i, h) in headings.iter().enumerate() {
            samples.extend(diverse_samples(h.clone(), 200 + i, 20));
        }

        let classifier = HeadingClassifier::build(samples_iter(&samples));

        assert_eq!(classifier.body, body);
        // Capped at 4 levels (H1..H4).
        assert_eq!(classifier.levels.len(), 4);

        // The 4 largest sizes should be picked: headings[5] (22pt) → H1 down
        // to headings[2] (16pt) → H4. headings[1] and headings[0] fall off.
        assert_eq!(classifier.classify(&headings[5]), Some(1));
        assert_eq!(classifier.classify(&headings[4]), Some(2));
        assert_eq!(classifier.classify(&headings[3]), Some(3));
        assert_eq!(classifier.classify(&headings[2]), Some(4));
        assert_eq!(classifier.classify(&headings[1]), None);
        assert_eq!(classifier.classify(&headings[0]), None);
    }

    #[test]
    fn rare_sizes_filtered_out() {
        // body=100k chars → min_heading_chars = max(50, 100000/5000) = 50.
        // h_real has 500 chars → kept. h_rare has 7 chars → dropped.
        let body = FontSignature::new(10.0, false, false);
        let h_real = FontSignature::new(20.0, true, false);
        let h_rare = FontSignature::new(16.0, false, false);

        let mut samples = diverse_samples(body.clone(), 100_000, 50);
        samples.extend(diverse_samples(h_real.clone(), 500, 20));
        samples.extend(diverse_samples(h_rare.clone(), 7, 7));

        let classifier = HeadingClassifier::build(samples_iter(&samples));

        assert_eq!(classifier.body, body);
        assert_eq!(classifier.levels.len(), 1, "rare h_rare should be filtered");
        assert_eq!(classifier.classify(&h_real), Some(1));
        assert_eq!(classifier.classify(&h_rare), None);
    }

    #[test]
    fn rare_size_floor_protects_short_docs() {
        // For a small doc (body=100 chars), the relative ratio gives 0; the
        // floor (50) still applies, so a 30-char heading still gets dropped.
        let body = FontSignature::new(10.0, false, false);
        let h = FontSignature::new(20.0, false, false);
        let mut samples = diverse_samples(body.clone(), 100, 50);
        samples.extend(diverse_samples(h.clone(), 30, 15));
        let classifier = HeadingClassifier::build(samples_iter(&samples));
        assert_eq!(classifier.classify(&h), None);
    }

    #[test]
    fn running_header_font_dropped() {
        // A "PŘEDPIS L14"-style banner repeated many times at a heading-sized
        // font. All chars at that level come from one repeated string, so the
        // diversity audit should drop it.
        let body = FontSignature::new(12.0, false, false);
        let bogus_h = FontSignature::new(14.0, true, false);
        let mut samples = diverse_samples(body.clone(), 10_000, 50);
        // 200 repetitions of the same 11-char string => 2200 chars all
        // attributed to one trimmed text => ratio = 1.0, drop the level.
        let banner = "PŘEDPIS L14".to_string();
        for _ in 0..200 {
            samples.push((bogus_h.clone(), banner.clone()));
        }

        let classifier = HeadingClassifier::build(samples_iter(&samples));
        assert_eq!(classifier.body, body);
        assert_eq!(
            classifier.classify(&bogus_h),
            None,
            "running header font should be dropped by the diversity audit"
        );
    }

    #[test]
    fn diverse_headings_kept() {
        // 20 distinct headings (50 chars each) at a heading-sized font — they
        // pass the diversity audit and the level is retained.
        let body = FontSignature::new(12.0, false, false);
        let h = FontSignature::new(16.0, true, false);

        let mut samples = diverse_samples(body.clone(), 10_000, 50);
        // 20 distinct strings, 50 chars each, all under one size_bucket.
        for i in 0..20 {
            let txt = format!("Heading number {:02} title", i);
            // Pad to ensure 50 chars.
            let padded = format!("{:<50}", txt);
            samples.push((h.clone(), padded));
        }

        let classifier = HeadingClassifier::build(samples_iter(&samples));
        assert_eq!(classifier.body, body);
        assert_eq!(
            classifier.classify(&h),
            Some(1),
            "diverse heading family should be kept"
        );
    }

    #[test]
    fn size_bucket_rounding() {
        // Exact values: 12.0pt and 12.1pt both map to bucket 24 (12.0 * 2 = 24, 12.1 * 2 ≈ 24.2 -> 24).
        let a = FontSignature::new(12.0, false, false);
        let b = FontSignature::new(12.1, false, false);
        assert_eq!(a.size_bucket, 24);
        assert_eq!(b.size_bucket, 24);

        // 12.5pt -> bucket 25. Values near 12.5 should also land here.
        let c = FontSignature::new(12.5, false, false);
        assert_eq!(c.size_bucket, 25);

        // 13.0pt -> bucket 26.
        let d = FontSignature::new(13.0, false, false);
        assert_eq!(d.size_bucket, 26);

        // 12.25pt -> 12.25*2=24.5 -> f32::round rounds half away from zero -> 25.
        let e = FontSignature::new(12.25, false, false);
        assert_eq!(e.size_bucket, 25);

        // 0pt -> bucket 0 (clamped via max(0.0)).
        let f = FontSignature::new(0.0, false, false);
        assert_eq!(f.size_bucket, 0);
    }
}
