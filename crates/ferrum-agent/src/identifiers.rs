//! Identifier spans for a host's privacy layer
//! ([#74](https://github.com/Kauter1989/ferrum/issues/74)).
//!
//! Medical Harness de-identifies free text before it reaches a model, a log
//! or a report. FERRUM knows the study's real identifiers, so it can say
//! **where** they appear in a text — positions and kinds only, never the
//! values. The host masks the spans itself.
//!
//! This is a **library API, not an agent command**: a model must not be able
//! to test guessed names against the patient's identifiers. It is in neither
//! the MCP tool list nor `commands.json` (a test checks both).
//!
//! Matching is tolerant, biased toward recall (over-masking is safe for a
//! privacy layer): case-insensitive, Cyrillic and Latin transliterated into
//! one canonical form, name components in either order, initials
//! («Иванов И.И.»), inflected forms (a stem plus up to three letters), and
//! the common date formats of a DICOM `YYYYMMDD` birth date, in English and
//! Russian.

use std::path::Path;

use ferrum_domain::PatientIdentifiers;

use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode};

/// What kind of identifier a span marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierKind {
    /// The patient name (any component, order, initials or inflection).
    PatientName,
    /// The patient ID.
    PatientId,
    /// The birth date, in any of the supported formats.
    BirthDate,
    /// The study's accession number.
    AccessionNumber,
}

/// One identifier found in the scanned text: byte offsets into it and the
/// kind. **Never the value** — the host already has the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// First byte of the identifier in the text.
    pub start: usize,
    /// One past the last byte.
    pub end: usize,
    /// What kind of identifier it is.
    pub kind: IdentifierKind,
}

/// Scans `text` for the identifiers of the study in `workspace`: positions
/// and kinds only, never values. The workspace's source and every recorded
/// file are checked against the operator's read roots, and the sources are
/// hash-verified, exactly as `Study::load` does — the identifiers are read
/// from the recorded files, never from a fresh walk of the source folder
/// (a planted symlink or file must not drive the masking). A NIfTI study
/// carries no identifiers, so the result is empty.
pub fn scan_identifiers(config: &AgentConfig, workspace: &Path, text: &str) -> Result<Vec<Span>, AgentError> {
    let root = config.resolve_workspace(workspace)?;
    if !root.join(ferrum_io::workspace::files::MANIFEST).exists() {
        return Err(AgentError::new(ErrorCode::NoStudy, format!("{} is not a workspace", root.display()))
            .hint("open a study first: study open --workspace <ws> <path>"));
    }
    let ws = ferrum_io::Workspace::open(&root)?;
    let src = ws.manifest().source.clone();
    crate::study::refuse_parent_components(&src.path)?;
    config.check_source(&src.path)?;
    // every recorded file path is checked before it is read (as in #82)
    let files = src.file_paths();
    for path in &files {
        crate::study::refuse_parent_components(path)?;
        config.check_source(path)?;
    }
    ws.verify_sources()?;
    let ids = ferrum_io::read_patient_identifiers_of_files(&files)?;
    Ok(spans_of(text, &ids))
}

/// One character of the folded text, with the byte range of the original
/// character it came from (a Cyrillic character may fold to several).
#[derive(Clone, Copy)]
struct Folded {
    ch: char,
    start: usize,
    end: usize,
}

/// Folds text into a canonical lowercase form: case-insensitive, Cyrillic
/// transliterated to Latin, so both scripts match in one space.
fn fold(text: &str) -> Vec<Folded> {
    let mut out = Vec::new();
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        for lower in c.to_lowercase() {
            for f in fold_char(lower) {
                out.push(Folded { ch: f, start: i, end });
            }
        }
    }
    // Russian routinely writes е for ё: canonicalise the yo digraph to e
    // so "Пётр", "Петр", "Petr" and "Pyotr" meet in one form (recall-biased).
    let mut merged: Vec<Folded> = Vec::with_capacity(out.len());
    let mut k = 0;
    while k < out.len() {
        if out[k].ch == 'y' && k + 1 < out.len() && out[k + 1].ch == 'o' {
            merged.push(Folded { ch: 'e', start: out[k].start, end: out[k + 1].end });
            k += 2;
        } else {
            merged.push(out[k]);
            k += 1;
        }
    }
    merged
}

/// The canonical form of one lowercase character: Cyrillic transliterated
/// (digraphs included), Latin diacritics stripped to the base letter,
/// combining marks dropped, everything else kept.
fn fold_char(c: char) -> Vec<char> {
    // a combining mark (e.g. the dot of 'İ'.to_lowercase()) carries no letter
    if ('\u{0300}'..='\u{036f}').contains(&c) {
        return Vec::new();
    }
    let s: &str = match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' | 'э' => "e",
        'ё' => "yo",
        'ж' => "zh",
        'з' => "z",
        'и' | 'й' => "i",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ъ' | 'ь' => "",
        'ы' => "y",
        'ю' => "yu",
        'я' => "ya",
        'à' | 'á' | 'â' | 'ä' | 'å' | 'ã' => "a",
        'è' | 'é' | 'ê' | 'ë' => "e",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' | 'ø' => "o",
        'ù' | 'ú' | 'û' | 'ü' => "u",
        'ñ' => "n",
        'ç' => "c",
        'ß' => "ss",
        'ý' | 'ÿ' => "y",
        'æ' => "ae",
        'œ' => "oe",
        _ => return vec![c],
    };
    s.chars().collect()
}

/// A word of the folded text: a maximal run of letters and digits, with its
/// indices into the folded sequence.
struct Word {
    start: usize,
    end: usize,
}

fn words(folded: &[Folded]) -> Vec<Word> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < folded.len() {
        if folded[i].ch.is_ascii_alphanumeric() {
            let start = i;
            while i < folded.len() && folded[i].ch.is_ascii_alphanumeric() {
                i += 1;
            }
            out.push(Word { start, end: i });
        } else {
            i += 1;
        }
    }
    out
}

fn word_text(folded: &[Folded], w: &Word) -> String {
    folded[w.start..w.end].iter().map(|f| f.ch).collect()
}

/// The original byte range a folded word covers.
fn word_bytes(folded: &[Folded], w: &Word) -> (usize, usize) {
    (folded[w.start].start, folded[w.end - 1].end)
}

/// The name components of a DICOM PN (`FAMILY^GIVEN^MIDDLE^^SUFFIX`),
/// folded; prefix and suffix are not identifiers people write.
fn name_components(name: &str) -> Vec<String> {
    name.split('^').take(3).map(|c| fold(c.trim()).iter().map(|f| f.ch).collect()).collect()
}

/// Whether a word is a component or an inflected form of it: the component
/// itself, or its stem (minus up to the last two letters, at least three)
/// plus up to three more letters. Short components match as prefixes.
fn word_matches(word: &str, comp: &str) -> bool {
    if comp.is_empty() {
        return false;
    }
    if word == comp {
        return true;
    }
    let n = comp.chars().count();
    let stem_len = if n >= 5 { n - 2 } else { n };
    let stem: String = comp.chars().take(stem_len).collect();
    let wlen = word.chars().count();
    wlen >= stem_len && wlen <= n + 3 && word.starts_with(&stem)
}

/// All spans of the identifiers in `text`, overlapping ones merged into
/// one so no part of an identifier stays unmasked between two detections.
fn spans_of(text: &str, ids: &PatientIdentifiers) -> Vec<Span> {
    let folded = fold(text);
    let ws = words(&folded);
    let mut spans = Vec::new();
    let comps = name_components(&ids.name);
    name_spans(&folded, &ws, &comps, &mut spans);
    value_spans(&folded, &ids.patient_id, IdentifierKind::PatientId, &mut spans);
    value_spans(&folded, &ids.accession_number, IdentifierKind::AccessionNumber, &mut spans);
    date_spans(&folded, &ids.birth_date, &mut spans);
    resolve_overlaps(spans)
}

/// The name spans: a component alone, a pair or triple in any order, and
/// initials next to the family name.
fn name_spans(folded: &[Folded], ws: &[Word], comps: &[String], out: &mut Vec<Span>) {
    let empty = String::new();
    let family = comps.first().unwrap_or(&empty);
    let given = comps.get(1).unwrap_or(&empty);
    let middle = comps.get(2).unwrap_or(&empty);
    for (i, w) in ws.iter().enumerate() {
        let text = word_text(folded, w);
        let alone = word_matches(&text, family) || word_matches(&text, given) || word_matches(&text, middle);
        if !alone {
            continue;
        }
        let (mut start, mut end) = word_bytes(folded, w);
        // a pair or triple of components in any order extends the span
        if let Some((s, e)) = group_span(folded, ws, i, comps) {
            start = s;
            end = e;
        }
        // initials after the family name: "Ivanov I.I."
        if word_matches(&text, family) {
            if let Some((s, e)) = initials_after(folded, ws, i, given, middle) {
                start = start.min(s);
                end = end.max(e);
            }
        }
        // initials before the family name: "I. Ivanov", "I.I. Ivanov"
        if let Some((s, e)) = initials_before(folded, ws, i, family, given, middle) {
            start = start.min(s);
            end = end.max(e);
        }
        out.push(Span { start, end, kind: IdentifierKind::PatientName });
    }
}

/// A run of two or three words matching distinct components (any order).
fn group_span(folded: &[Folded], ws: &[Word], i: usize, comps: &[String]) -> Option<(usize, usize)> {
    for n in [3, 2] {
        if i + n > ws.len() {
            continue;
        }
        let texts: Vec<String> = ws[i..i + n].iter().map(|w| word_text(folded, w)).collect();
        if assigns(&texts, comps) {
            let (s, _) = word_bytes(folded, &ws[i]);
            let (_, e) = word_bytes(folded, &ws[i + n - 1]);
            return Some((s, e));
        }
    }
    None
}

/// Whether each word matches a distinct component (injective assignment).
/// Backtracking, not first-fit: "Ivanovich Ivan Ivanov" must assign
/// middle–family–given, which a greedy pass misses.
fn assigns(texts: &[String], comps: &[String]) -> bool {
    fn fit(ti: usize, texts: &[String], comps: &[String], used: &mut [bool]) -> bool {
        if ti == texts.len() {
            return true;
        }
        for (j, c) in comps.iter().enumerate() {
            if !used[j] && word_matches(&texts[ti], c) {
                used[j] = true;
                if fit(ti + 1, texts, comps, used) {
                    return true;
                }
                used[j] = false;
            }
        }
        false
    }
    fit(0, texts, comps, &mut vec![false; comps.len()])
}

/// One or two single-letter words after the family word, matching the
/// initials of the given and middle names.
fn initials_after(folded: &[Folded], ws: &[Word], i: usize, given: &str, middle: &str) -> Option<(usize, usize)> {
    let inits = [given.chars().next(), middle.chars().next()];
    let mut j = i + 1;
    let mut k = 0;
    while j < ws.len() && k < 2 {
        let t = word_text(folded, &ws[j]);
        let mut chars = t.chars();
        let (Some(first), None) = (chars.next(), chars.next()) else { break };
        if inits[k] != Some(first) {
            break;
        }
        j += 1;
        k += 1;
    }
    if k == 0 {
        return None;
    }
    let (s, _) = word_bytes(folded, &ws[i + 1]);
    let mut e = word_bytes(folded, &ws[j - 1]).1;
    // the dots of "I.I." belong to the initials
    let mut p = ws[j - 1].end;
    while p < folded.len() && folded[p].ch == '.' {
        e = folded[p].end;
        p += 1;
    }
    Some((s, e))
}

/// One or two initial words before the family word at `i`.
fn initials_before(
    folded: &[Folded],
    ws: &[Word],
    i: usize,
    family: &str,
    given: &str,
    middle: &str,
) -> Option<(usize, usize)> {
    if !word_matches(&word_text(folded, &ws[i]), family) {
        return None;
    }
    let inits = [given.chars().next(), middle.chars().next()];
    // one initial directly before, or two (given then middle)
    for n in [2, 1] {
        if i < n {
            continue;
        }
        let mut ok = true;
        for k in 0..n {
            let t = word_text(folded, &ws[i - n + k]);
            let mut chars = t.chars();
            let (Some(first), None) = (chars.next(), chars.next()) else {
                ok = false;
                break;
            };
            if inits[k] != Some(first) {
                ok = false;
                break;
            }
        }
        if ok {
            let (s, _) = word_bytes(folded, &ws[i - n]);
            let (_, e) = word_bytes(folded, &ws[i - 1]);
            return Some((s, e));
        }
    }
    None
}

/// A value (ID, accession) found with word boundaries, case-insensitive.
fn value_spans(folded: &[Folded], value: &str, kind: IdentifierKind, out: &mut Vec<Span>) {
    let v: Vec<char> = fold(value.trim()).iter().map(|f| f.ch).collect();
    if v.is_empty() || v.len() > folded.len() {
        return;
    }
    for i in 0..=folded.len().saturating_sub(v.len()) {
        if !folded[i..i + v.len()].iter().map(|f| f.ch).eq(v.iter().copied()) {
            continue;
        }
        let before = i == 0 || !folded[i - 1].ch.is_ascii_alphanumeric();
        let after = i + v.len() == folded.len() || !folded[i + v.len()].ch.is_ascii_alphanumeric();
        if before && after {
            out.push(Span { start: folded[i].start, end: folded[i + v.len() - 1].end, kind });
        }
    }
}

/// The birth date (`YYYYMMDD`) in the common formats, English and Russian.
fn date_spans(folded: &[Folded], birth: &str, out: &mut Vec<Span>) {
    let b = birth.trim();
    if b.len() != 8 || !b.chars().all(|c| c.is_ascii_digit()) {
        return;
    }
    let (y, m, d) = (&b[0..4], &b[4..6], &b[6..8]);
    let (mi, di) = (m.parse::<usize>().unwrap_or(0), d.parse::<usize>().unwrap_or(0));
    // both zero-padded and plain day/month: "1.1.1970" is the most common
    // Russian numeric spelling
    let (mn, dn) = (mi.to_string(), di.to_string());
    let mut forms = vec![format!("{y}{m}{d}")];
    for (mm, dd) in [(m, d), (mn.as_str(), dn.as_str())] {
        forms.push(format!("{y}-{mm}-{dd}"));
        forms.push(format!("{y}.{mm}.{dd}"));
        forms.push(format!("{dd}.{mm}.{y}"));
        forms.push(format!("{dd}/{mm}/{y}"));
        forms.push(format!("{dd}-{mm}-{y}"));
        forms.push(format!("{mm}/{dd}/{y}"));
        forms.push(format!("{mm}-{dd}/{y}"));
    }
    if (1..=12).contains(&mi) && (1..=31).contains(&di) {
        for day in [dn.as_str(), d] {
            for name in month_names(mi) {
                forms.push(format!("{day} {name} {y}"));
                forms.push(format!("{name} {day} {y}"));
                forms.push(format!("{day} {name}, {y}"));
                forms.push(format!("{name} {day}, {y}"));
            }
        }
    }
    for form in forms {
        value_spans(folded, &form, IdentifierKind::BirthDate, out);
    }
}

/// The month's names, English and Russian (nominative and genitive),
/// already folded.
fn month_names(m: usize) -> Vec<String> {
    const EN: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    const RU: [&str; 12] = [
        "yanvar", "fevral", "mart", "aprel", "mai", "iyun", "iyul", "avgust", "sentyabr", "oktyabr", "noyabr", "dekabr",
    ];
    const RU_GEN: [&str; 12] = [
        "yanvarya",
        "fevralya",
        "marta",
        "aprelya",
        "maya",
        "iyunya",
        "iyulya",
        "avgusta",
        "sentyabrya",
        "oktyabrya",
        "noyabrya",
        "dekabrya",
    ];
    let i = m - 1;
    let mut out = vec![EN[i].to_owned(), RU[i].to_owned(), RU_GEN[i].to_owned()];
    out.push(EN[i][..3].to_owned());
    out
}

/// Sorts by start and merges overlapping spans into one (the first kind):
/// dropping an overlap would leave part of an identifier unmasked between
/// two detections ("Ivanov I. Ivan" — the given name after the initial).
fn resolve_overlaps(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by(|a, b| a.start.cmp(&b.start).then((b.end - b.start).cmp(&(a.end - a.start))));
    let mut out: Vec<Span> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(last) if s.start < last.end => last.end = last.end.max(s.end),
            _ => out.push(s),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(name: &str) -> PatientIdentifiers {
        PatientIdentifiers {
            name: name.to_owned(),
            patient_id: "MRN-4711".into(),
            birth_date: "19700101".into(),
            accession_number: "ACC-0815".into(),
        }
    }

    // covers 27.1-a
    #[test]
    fn empty_identifiers_give_no_spans() {
        let spans = spans_of("Ivanov Ivan, MRN-4711, 01.01.1970", &PatientIdentifiers::default());
        assert!(spans.is_empty(), "{spans:?}");
    }

    // covers 27.1-a
    #[test]
    fn a_full_name_is_one_span_not_three() {
        let text = "Ivanov Ivan Ivanovich arrived";
        let spans = spans_of(text, &ids("IVANOV^IVAN^IVANOVICH"));
        assert_eq!(spans, vec![Span { start: 0, end: 21, kind: IdentifierKind::PatientName }]);
    }

    // covers 27.1-a
    #[test]
    fn initials_keep_their_dots() {
        let text = "Ivanov I.I. arrived";
        let spans = spans_of(text, &ids("IVANOV^IVAN^IVANOVICH"));
        assert_eq!(spans, vec![Span { start: 0, end: 11, kind: IdentifierKind::PatientName }]);
    }

    // covers 27.1-a
    #[test]
    fn an_inflected_family_name_alone_is_found() {
        let text = "карта Иванова";
        let spans = spans_of(text, &ids("IVANOV^IVAN^IVANOVICH"));
        let start = text.find("Иванова").unwrap();
        assert_eq!(spans, vec![Span { start, end: start + "Иванова".len(), kind: IdentifierKind::PatientName }]);
    }

    // covers 27.1-a
    #[test]
    fn a_short_component_matches_as_a_prefix() {
        // "Li" is a real family name; over-masking is the safe direction
        let text = "Liu and Li";
        let spans = spans_of(text, &ids("LI^WEI"));
        assert_eq!(spans.len(), 2, "{spans:?}");
    }

    // covers 27.1-a
    #[test]
    fn overlapping_detections_merge_into_one_span() {
        // the given name after the initial must not stay unmasked
        let text = "Ivanov I. Ivan";
        let spans = spans_of(text, &ids("IVANOV^IVAN^IVANOVICH"));
        assert_eq!(spans, vec![Span { start: 0, end: 14, kind: IdentifierKind::PatientName }]);
        let text = "Ivanov Ivan I.";
        let spans = spans_of(text, &ids("IVANOV^IVAN^IVANOVICH"));
        assert_eq!(spans, vec![Span { start: 0, end: 14, kind: IdentifierKind::PatientName }]);
    }

    // covers 27.1-a
    #[test]
    fn a_reordered_triple_is_one_span() {
        let text = "Ivanovich Ivan Ivanov";
        let spans = spans_of(text, &ids("IVANOV^IVAN^IVANOVICH"));
        assert_eq!(spans, vec![Span { start: 0, end: 21, kind: IdentifierKind::PatientName }]);
    }

    // covers 27.1-a
    #[test]
    fn yo_and_e_are_one_letter() {
        let text = "Иванов Петр";
        let spans = spans_of(text, &ids("IVANOV^ПЁТР"));
        let start = text.find("Иванов").unwrap();
        assert_eq!(spans, vec![Span { start, end: text.len(), kind: IdentifierKind::PatientName }]);
        let text = "Ivanov Pyotr";
        let spans = spans_of(text, &ids("IVANOV^ПЁТР"));
        assert_eq!(spans, vec![Span { start: 0, end: 12, kind: IdentifierKind::PatientName }]);
        let text = "Иванов Пётр";
        let spans = spans_of(text, &ids("IVANOV^ПЕТР"));
        let start = text.find("Иванов").unwrap();
        assert_eq!(spans, vec![Span { start, end: text.len(), kind: IdentifierKind::PatientName }]);
    }

    // covers 27.1-a
    #[test]
    fn latin_diacritics_fold_to_the_base_letter() {
        let text = "Müller Hans";
        let spans = spans_of(text, &ids("MÜLLER^HANS"));
        assert_eq!(spans, vec![Span { start: 0, end: text.len(), kind: IdentifierKind::PatientName }]);
        let text = "MULLER";
        let spans = spans_of(text, &ids("MÜLLER^HANS"));
        assert_eq!(spans, vec![Span { start: 0, end: 6, kind: IdentifierKind::PatientName }]);
    }

    // covers 27.1-a
    #[test]
    fn unpadded_numeric_dates_are_found() {
        for form in ["1.1.1970", "1/1/1970", "1970-1-1", "01 января 1970"] {
            let text = format!("дата: {form}.");
            let spans = spans_of(&text, &ids("IVANOV^IVAN"));
            let start = text.find(form).unwrap();
            assert_eq!(
                spans,
                vec![Span { start, end: start + form.len(), kind: IdentifierKind::BirthDate }],
                "format {form}"
            );
        }
    }

    // covers 27.1-a (security review of #110: a date overlapping the
    // initials span must not leave its tail unmasked)
    #[test]
    fn a_date_overlapping_the_name_span_is_merged() {
        let text = "Ivanov 1.1.1970";
        let spans = spans_of(text, &ids("IVANOV^1X^1Y"));
        assert_eq!(spans, vec![Span { start: 0, end: 15, kind: IdentifierKind::PatientName }]);
    }
}
