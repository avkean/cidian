use super::pinyin::{
    PATTERN_WORK, Pattern, PatternBudget, Pinyin, has_wildcard, mixed_matches, pattern,
};
use super::{MatchKind, Relevance, SearchIndex, Word, is_hanzi, pinyin};
use unicode_normalization::UnicodeNormalization;

impl SearchIndex {
    fn citation_matches(&self, word: &Word, query: &Pinyin) -> bool {
        let reading = Pinyin::parse(&word.pinyin);
        reading.matches_prefix(query, |syllable, tone| {
            let Some(character) = word.simplified.chars().nth(syllable) else {
                return false;
            };
            let mut tones = self
                .by_headword(&character.to_string())
                .iter()
                .filter_map(|&id| {
                    let citation = Pinyin::parse(&self.words[id as usize].pinyin);
                    let [(letters, own)] = citation.syllables.as_slice() else {
                        return None;
                    };
                    (*letters == reading.syllables[syllable].0 && *own != 5).then_some(*own)
                })
                .peekable();
            tones.peek().is_none() || tones.any(|own| own == tone)
        })
    }

    pub(super) fn tone_numbered(&self, text: &str) -> bool {
        let mut letters = String::new();
        let mut digits = false;
        for c in text.to_lowercase().nfd() {
            if c.is_ascii_digit() {
                if c > '5' || !self.syllable_run(&pinyin::letters(&letters)) {
                    return false;
                }
                digits = true;
                letters.clear();
            } else if c.is_ascii_alphabetic() {
                letters.push(c);
            } else if !(c == ':' || unicode_normalization::char::is_combining_mark(c)) {
                letters.clear();
            }
        }
        digits
    }

    fn syllable_run(&self, letters: &str) -> bool {
        let mut ends = vec![false; letters.len() + 1];
        ends[0] = true;
        for end in 1..=letters.len() {
            ends[end] =
                (0..end).any(|start| ends[start] && self.syllables.contains(&letters[start..end]));
        }
        !letters.is_empty() && ends[letters.len()]
    }

    pub(super) fn chinese_matches(
        &self,
        text: &str,
        tone_numbered: bool,
    ) -> (Vec<(Relevance, u32)>, bool) {
        self.chinese_with_budget(text, tone_numbered, PATTERN_WORK)
    }

    pub(super) fn chinese_with_budget(
        &self,
        text: &str,
        tone_numbered: bool,
        limit: usize,
    ) -> (Vec<(Relevance, u32)>, bool) {
        let mut budget = PatternBudget {
            remaining: limit,
            exhausted: false,
        };
        let has_hanzi = text.chars().any(is_hanzi);
        let has_latin = text.nfd().any(|c| c.is_ascii_alphabetic());
        let not_pinyin = not_pinyin(text, tone_numbered);
        let mut results = Vec::new();
        if has_hanzi || not_pinyin {
            let capitals = text.to_ascii_uppercase();
            let mut starts = vec![text];
            if not_pinyin && capitals != text {
                starts.push(&capitals);
            }
            for start in starts {
                for (word, ids) in self.headwords.range(start.to_string()..) {
                    if !word.starts_with(start) {
                        break;
                    }
                    let rank = Relevance::new(if word == start {
                        MatchKind::Exact
                    } else {
                        MatchKind::Prefix
                    });
                    results.extend(ids.iter().map(|&id| (rank, id)));
                }
            }
        }
        if results.is_empty()
            && (has_hanzi || !text.chars().all(|c| c.is_alphabetic() || c.is_whitespace()))
        {
            let exact = Relevance::new(MatchKind::Exact);
            results.extend(self.exact(text).iter().map(|&id| (exact, id)));
        }
        if !results.is_empty() || (not_pinyin && !has_hanzi) {
            return (results, false);
        }
        if has_wildcard(text) || (has_hanzi && has_latin) {
            let pattern = pattern(text);
            let first_han = pattern.iter().find_map(|token| match token {
                Pattern::Han(c) => Some(*c),
                _ => None,
            });
            // bound the work before parsing; broad patterns have no useful answer and a truncated scan would mislead
            const MAX_PATTERN_CANDIDATES: usize = 10_000;
            let mut ids = Vec::new();
            let mut collect = |candidates: &[u32], budget: &mut PatternBudget| {
                for &id in candidates {
                    if !budget.take(1) {
                        return false;
                    }
                    ids.push(id);
                    if ids.len() > MAX_PATTERN_CANDIDATES {
                        budget.exhausted = true;
                        return false;
                    }
                }
                true
            };
            if let Some(c) = first_han {
                if let Some(candidates) = self.characters.get(&c) {
                    collect(candidates, &mut budget);
                }
            } else {
                let prefix = matches!(pattern.first(), Some(Pattern::Pinyin(_)));
                let Some(letters) = pattern.iter().find_map(|token| match token {
                    Pattern::Pinyin(query) if prefix || !query.letters.is_empty() => {
                        Some(&query.letters)
                    }
                    _ => None,
                }) else {
                    return (results, false);
                };
                let start = if prefix {
                    letters.clone()
                } else {
                    String::new()
                };
                for (key, candidates) in self.pinyin.range(start..) {
                    if prefix && !key.starts_with(letters) {
                        break;
                    }
                    if !budget.take(1) {
                        break;
                    }
                    if (prefix || key.contains(letters)) && !collect(candidates, &mut budget) {
                        break;
                    }
                }
            }
            if ids.len() > MAX_PATTERN_CANDIDATES {
                return (results, budget.exhausted);
            }
            for id in ids {
                if budget.exhausted {
                    break;
                }
                let word = &self.words[id as usize];
                let pinyin = Pinyin::parse(&word.pinyin);
                let matched = mixed_matches(&pattern, &word.simplified, &pinyin, &mut budget)
                    || word.traditional.iter().any(|form| {
                        // Without Han literals, only the form's length affects a match.
                        !budget.exhausted
                            && (first_han.is_some()
                                || form.chars().count() != word.simplified.chars().count())
                            && mixed_matches(&pattern, form, &pinyin, &mut budget)
                    });
                if matched {
                    results.push((Relevance::new(MatchKind::Pattern), id));
                }
            }
            return (results, budget.exhausted);
        }
        if has_hanzi {
            let first = text.chars().find(|c| is_hanzi(*c)).unwrap();
            for &id in self.characters.get(&first).into_iter().flatten() {
                let word = &self.words[id as usize];
                if word.simplified.contains(text)
                    || word.traditional.iter().any(|form| form.contains(text))
                {
                    results.push((Relevance::new(MatchKind::Contains), id));
                }
            }
            return (results, false);
        }
        let query = Pinyin::parse(text);
        if query.letters.is_empty() {
            return (results, false);
        }
        // the index key already proves an unmarked prefix, so only parse for tone, break and umlaut checks
        let plain =
            query.boundaries.len() < 2 && query.tones.is_empty() && query.umlauts.is_empty();
        for (key, ids) in self.pinyin.range(query.letters.clone()..) {
            if !key.starts_with(&query.letters) {
                break;
            }
            let kind = if key == &query.letters {
                MatchKind::Exact
            } else {
                MatchKind::Prefix
            };
            results.extend(
                ids.iter()
                    .filter(|&&id| plain || self.citation_matches(&self.words[id as usize], &query))
                    .map(|&id| (Relevance::new(kind), id)),
            );
        }
        (results, false)
    }
}

pub(super) fn not_pinyin(text: &str, tone_numbered: bool) -> bool {
    !tone_numbered
        && text.nfd().any(|c| c.is_ascii_alphabetic())
        && text.chars().any(|c| c.is_ascii_digit())
}
