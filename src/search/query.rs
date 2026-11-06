use super::chinese::not_pinyin;
use super::english::{
    RANKING_WORK, abbreviation, fold_english, fulltext_query, joined, may_be_english, tokens,
    with_apostrophes,
};
use super::pinyin::is_wildcard;
use super::{
    Line, MatchKind, NEWS_CORROBORATION_FLOOR, Ranked, RankedCache, RankedKey, Relevance,
    SearchIndex, SearchQuery, SearchResponse, is_hanzi, lexicon, pinyin,
};
use std::collections::HashSet;
use std::sync::Arc;

const REGROUP_WORDS: usize = 4;
const REGROUP_TRIES: usize = 8;
enum Replace {
    Respell,
    Regroup,
    Related,
}

enum FallbackStep {
    Try(Replace),
    CompleteTail,
}

impl SearchIndex {
    fn used(&self, word: &str) -> bool {
        self.english.contains_key(word)
            || self.usual.contains_key(word)
            || lexicon::lexicon()
                .forms(word)
                .iter()
                .any(|form| self.english.contains_key(form))
    }

    fn respelled(&self, word: &str) -> Option<&str> {
        let lexicon = lexicon::lexicon();
        if lexicon.knows(word) {
            lexicon
                .adverb_bases(word)
                .iter()
                .copied()
                .find(|base| self.used(base))
        } else {
            self.speller.suggest(word)
        }
    }

    fn related_word(&self, word: &str) -> Option<(&'static str, Vec<(Relevance, u32)>)> {
        lexicon::lexicon()
            .related(word)
            .into_iter()
            .find_map(|base| {
                let rows = self.english_matches(base, false);
                let found = rows.iter().any(|(rank, _)| {
                    (MatchKind::WholeGloss..=MatchKind::GlossHead).contains(&rank.kind)
                        && rank.form_cost == 0
                });
                found.then_some((base, rows))
            })
    }

    fn regrouped(&self, text: &str) -> Option<(String, Vec<(Relevance, u32)>)> {
        let lexicon = lexicon::lexicon();
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        if words.len() > REGROUP_WORDS
            || !words
                .iter()
                .all(|word| word.bytes().all(|b| b.is_ascii_lowercase()))
        {
            return None;
        }
        let known = |word: &str| self.used(word) || lexicon.knows(word);
        let with = |range: std::ops::Range<usize>, replacement: String| {
            let mut found = words[..range.start].to_vec();
            found.push(replacement);
            found.extend_from_slice(&words[range.end..]);
            found.join(" ")
        };
        let mut candidates = Vec::new();
        for (at, word) in words.iter().enumerate() {
            if !self.used(word) {
                for cut in 2..word.len().saturating_sub(1) {
                    let (left, right) = word.split_at(cut);
                    if known(left) && known(right) {
                        let rarest = lexicon.count(left).min(lexicon.count(right));
                        candidates.push((rarest, with(at..at + 1, format!("{left} {right}"))));
                    }
                }
            }
            if let Some(next) = words.get(at + 1) {
                let joined = format!("{word}{next}");
                if known(&joined) {
                    candidates.push((lexicon.count(&joined), with(at..at + 2, joined)));
                }
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        candidates.truncate(REGROUP_TRIES);
        candidates.into_iter().find_map(|(_, text)| {
            let rows = self.english_matches(&text, false);
            rows.iter()
                .any(|(rank, _)| rank.shown_whole())
                .then_some((text, rows))
        })
    }

    fn correct<'a>(
        &'a self,
        text: &str,
        mut replace: impl FnMut(&str) -> Option<&'a str>,
    ) -> Option<String> {
        let mut corrected = String::with_capacity(text.len());
        let mut last = 0;
        for word in text.split(|c: char| !c.is_alphanumeric() && !matches!(c, '\'' | '’')) {
            let lower = word.to_ascii_lowercase();
            if !word.bytes().all(|b| b.is_ascii_alphabetic())
                || abbreviation(word)
                || self.used(&lower)
            {
                continue;
            }
            if let Some(suggestion) = replace(&lower) {
                let start = word.as_ptr() as usize - text.as_ptr() as usize;
                corrected.push_str(&text[last..start]);
                corrected.push_str(suggestion);
                last = start + word.len();
            }
        }
        (last > 0).then(|| corrected + &text[last..])
    }

    // Searching as typed blocks further replacements, but phrase groups can still reject the retry.
    fn replace(&self, query: &SearchQuery, text: &str, kind: Replace) -> Option<Ranked> {
        let (replaced, rows) = match kind {
            Replace::Respell => (self.correct(text, |word| self.respelled(word))?, None),
            Replace::Regroup => self
                .regrouped(text)
                .map(|(text, rows)| (text, Some(rows)))?,
            Replace::Related => {
                let mut probe = None;
                let related = self.correct(text, |word| {
                    probe = self.related_word(word);
                    probe.as_ref().map(|(base, _)| *base)
                })?;
                // Punctuation or other tokens make this a different search from the bare probe.
                let rows = probe.filter(|(base, _)| *base == related);
                (related, rows.map(|(_, rows)| rows))
            }
        };
        // a respelling keeps the requested mode, so auto can still route it; the others are English words
        let mode = if let Replace::Respell = kind {
            query.mode.as_str()
        } else {
            "english"
        };
        let replacement = SearchQuery {
            q: replaced.clone(),
            mode: mode.into(),
            as_typed: true,
            ..query.clone()
        };
        let mut found = match rows {
            Some(rows) => {
                self.complete(&replacement, &replaced, "english", Ranked::default(), rows)
            }
            None => self.rank_query(&replacement),
        };
        // a replaced phrase that only splits into words is no better than the typed one
        if found.matches.is_empty() || !found.segments.is_empty() {
            return None;
        }
        if let Replace::Related = kind {
            found.related = Some(replaced);
        } else {
            found.corrected = Some(replaced);
        }
        Some(found)
    }

    pub fn search(&self, query: &SearchQuery) -> SearchResponse {
        self.page(query, &self.cached_ranked(query))
    }

    pub(super) fn cached_ranked(&self, query: &SearchQuery) -> Arc<Ranked> {
        let folded = fold_width(&query.q);
        let trimmed = folded.trim();
        let prepared = self.cache_query(query);
        let key = if prepared.mode == "fulltext" {
            RankedKey::Search(fulltext_query(&prepared.q), "fulltext".into(), false)
        } else {
            RankedKey::Search(prepared.q, prepared.mode, prepared.as_typed)
        };
        RankedCache::get_or_compute(&self.ranked, key, || self.rank_prepared(query, trimmed))
    }

    pub(crate) fn cache_query(&self, query: &SearchQuery) -> SearchQuery {
        let folded = fold_width(&query.q);
        let trimmed = folded.trim();
        let mode = if trimmed.starts_with('#') {
            "fulltext"
        } else {
            query.search_mode()
        };
        let mut text = tidy_chinese(trimmed.trim_start_matches('#').trim());
        // English treats these as trailing separators. Keep a separator: a bare word can
        // enable initials or regrouping. Corrections must retain their typed punctuation.
        if mode == "english"
            && text.ends_with(['!', '?'])
            && !text.chars().any(is_hanzi)
            && with_apostrophes(&text).is_none()
            && (query.as_typed
                || self
                    .correct(&text, |word| (!word.is_empty()).then_some(""))
                    .is_none())
            && self.by_headword(&text).is_empty()
        {
            let plain = text.trim_end_matches(['!', '?']);
            let canonical = format!("{plain}!");
            if self.by_headword(&canonical).is_empty() {
                text = canonical;
            }
        }
        SearchQuery {
            q: text,
            mode: mode.into(),
            as_typed: mode != "fulltext" && query.as_typed,
            ..query.clone().normalized()
        }
    }

    pub(super) fn rank_query(&self, query: &SearchQuery) -> Ranked {
        let folded = fold_width(&query.q);
        self.rank_prepared(query, folded.trim())
    }

    fn rank_prepared(&self, query: &SearchQuery, trimmed: &str) -> Ranked {
        let mode = if trimmed.starts_with('#') {
            "fulltext"
        } else {
            query.search_mode()
        };
        let text = &tidy_chinese(trimmed.trim_start_matches('#').trim());
        let mut ranked = Ranked {
            mode: mode.into(),
            ..Ranked::default()
        };
        if text.is_empty() {
            return ranked;
        }
        if matches!(mode, "auto" | "english")
            && !query.as_typed
            && may_be_english(text)
            && let Some(written) = with_apostrophes(text)
        {
            let mut found = self.rank_query(&SearchQuery {
                q: written.clone(),
                ..query.clone()
            });
            found.corrected.get_or_insert(written);
            return found;
        }
        let (matches, mode, truncated) = match mode {
            "english" => {
                let (matches, truncated) = self.english_with_budget(text, false, RANKING_WORK);
                (matches, mode, truncated)
            }
            "fulltext" => {
                let (matches, truncated) = self.fulltext_matches(text);
                (matches, mode, truncated)
            }
            "chinese" => {
                let (matches, truncated) = self.chinese_matches(text, self.tone_numbered(text));
                (matches, mode, truncated)
            }
            _ => {
                let (matches, mode, other, truncated) = self.auto_matches(text);
                ranked.line = other;
                (matches, mode, truncated)
            }
        };
        let mut ranked = self.complete(query, text, mode, ranked, matches);
        ranked.truncated |= truncated;
        ranked
    }

    fn complete(
        &self,
        query: &SearchQuery,
        text: &str,
        mode: &str,
        mut ranked: Ranked,
        mut matches: Vec<(Relevance, u32)>,
    ) -> Ranked {
        let tail = matches!(mode, "english" | "mixed") && !text.chars().any(is_hanzi);
        let lexical = tail.then(|| self.lexical_query(text));
        if matches!(mode, "english" | "mixed") {
            ranked.truncated = self.keep_best_parts(text, &mut matches, lexical.as_ref());
        }
        self.rank(&mut matches, mode != "chinese");
        if mode == "chinese" && !text.chars().any(is_hanzi) {
            for group in matches.chunk_by_mut(|a, b| a.0.kind == b.0.kind) {
                self.order_characters(group, |item| item.1);
            }
        }
        let english = mode == "english" && !query.as_typed;
        let named = matches
            .iter()
            .any(|(rank, _)| (MatchKind::Usual..=MatchKind::Mention).contains(&rank.kind));
        let spelled_out = if english {
            self.spelled_out(text, named)
        } else {
            &[]
        };
        for step in [
            FallbackStep::Try(Replace::Respell),
            FallbackStep::CompleteTail,
            FallbackStep::Try(Replace::Regroup),
            FallbackStep::Try(Replace::Related),
        ] {
            match step {
                FallbackStep::CompleteTail => {
                    // Support can supply an answer, so complete it before the empty-result tries.
                    if let Some(lexical) = &lexical {
                        self.rank_tail(text, &mut matches, lexical);
                    }
                    self.add_spelled_out(&mut matches, spelled_out);
                }
                FallbackStep::Try(kind) => {
                    let eligible = english
                        && match kind {
                            Replace::Respell => !named && spelled_out.is_empty(),
                            Replace::Regroup => matches.is_empty(),
                            Replace::Related => {
                                matches.is_empty() && text.split_whitespace().count() == 1
                            }
                        };
                    if eligible && let Some(found) = self.replace(query, text, kind) {
                        return found;
                    }
                }
            }
        }
        if matches!(mode, "english" | "mixed") && matches.iter().any(|(rank, _)| rank.relevant()) {
            let before = matches.len();
            matches.retain(|(rank, _)| rank.relevant());
            ranked.truncated &= matches.len() == before;
        }
        if mode == "english" && matches.is_empty() {
            self.split_phrase(
                text,
                &mut matches,
                &mut ranked.segments,
                &mut ranked.unfound,
            );
        }
        if mode == "chinese" && self.exact(text).is_empty() {
            self.break_down(text, &mut matches, &mut ranked.segments);
        }
        ranked.mode = mode.into();
        ranked.text = text.into();
        ranked.matches = matches;
        ranked
    }

    pub(super) fn auto_matches(
        &self,
        text: &str,
    ) -> (Vec<(Relevance, u32)>, &'static str, Line, bool) {
        let tone_numbered = self.tone_numbered(text);
        let (mut matches, chinese_truncated) = self.chinese_matches(text, tone_numbered);
        let latin_only =
            !text.chars().any(is_hanzi) && text.chars().any(|c| c.is_ascii_alphabetic());
        if (!may_be_english(text) || tone_numbered) && !(matches.is_empty() && latin_only) {
            return (matches, "chinese", Line::None, chinese_truncated);
        }
        let (english, truncated) =
            self.english_with_budget(text, !matches.is_empty(), RANKING_WORK);
        let truncated = truncated || chinese_truncated;
        if matches.is_empty() {
            return (english, "english", Line::None, truncated);
        }
        let not_pinyin = not_pinyin(text, tone_numbered);
        if not_pinyin || !self.english_word(text, &english) {
            matches.extend(
                english
                    .into_iter()
                    .filter(|(rank, _)| rank.kind < MatchKind::Mention),
            );
            return (
                matches,
                "chinese",
                if not_pinyin {
                    Line::None
                } else {
                    Line::Capitals
                },
                truncated,
            );
        }
        // pinyin rows lead with the default meaning on purpose: they matched by sound, not by an english gloss
        for (rank, _) in &mut matches {
            *rank = Relevance::new(if rank.kind == MatchKind::Exact {
                MatchKind::Pinyin
            } else {
                MatchKind::PinyinPrefix
            });
        }
        let literal_noun = lexicon::lexicon().noun_lemma(&fold_english(text));
        let pinyin_ids: HashSet<u32> = matches
            .iter()
            .filter(|(rank, _)| rank.kind == MatchKind::Pinyin)
            .map(|(_, id)| *id)
            .collect();
        let english: Vec<_> = english
            .into_iter()
            .filter(|(rank, id)| {
                rank.kind < MatchKind::Mention
                    // a romanized name ("Song") is no evidence for the noun ("song")
                    && !(literal_noun && rank.proper && pinyin_ids.contains(id))
            })
            .collect();
        let confirmed_names: HashSet<u32> = english
            .iter()
            .filter(|(rank, _)| {
                rank.proper && rank.kind == MatchKind::WholeGloss && rank.shown_whole()
            })
            .map(|(_, id)| *id)
            .collect();
        for (rank, id) in &mut matches {
            if rank.kind == MatchKind::Pinyin && confirmed_names.contains(id) {
                rank.proper = true;
            }
        }
        let mode = if !english.is_empty() {
            "mixed"
        } else {
            "chinese"
        };
        let mut pinyin: Vec<u32> = if mode == "mixed" {
            pinyin_ids.into_iter().collect()
        } else {
            Vec::new()
        };
        self.sort(&mut pinyin);
        self.order_characters(&mut pinyin, |&id| id);
        let mut seen = HashSet::new();
        pinyin.retain(|&id| {
            let word = &self.words[id as usize];
            word.reading_corpus_frequency >= NEWS_CORROBORATION_FLOOR
                && seen.insert(word.simplified.as_str())
        });
        pinyin.truncate(5);
        matches.extend(english);
        (matches, mode, Line::Pinyin(pinyin), truncated)
    }

    fn english_word(&self, text: &str, matches: &[(Relevance, u32)]) -> bool {
        let lexicon = lexicon::lexicon();
        let letters = pinyin::letters(text);
        let folded = fold_english(&joined(text).unwrap_or_else(|| text.to_string()));
        let known = |word: &str| lexicon.knows(word) || self.initials.contains(word);
        let listed = tokens(&folded).all(known);
        // subtitle-only words need a shown meaning ("we"), not a search-only gloss ("-al" of 有)
        let common = tokens(&folded).all(|word| known(word) || self.more_english(word));
        let inflection = tokens(&folded).any(|word| lexicon.inflection(word));
        self.is_abbreviation(text)
            || matches.iter().any(|(rank, id)| {
                matches!(
                    rank.kind,
                    MatchKind::Usual | MatchKind::WholeGloss | MatchKind::LaterGloss
                ) && rank.form_cost <= u8::from(inflection)
                    && if rank.proper {
                        !self.words[*id as usize]
                            .pinyin
                            .split_whitespace()
                            .scan(String::new(), |start, syllable| {
                                start.push_str(&pinyin::letters(syllable));
                                Some(start.clone())
                            })
                            .any(|start| start == letters)
                    } else {
                        listed || (common && rank.shown_whole())
                    }
            })
    }

    // News counts include compound uses, so compare pinyin against jieba's word counts.
    fn more_english(&self, word: &str) -> bool {
        let pinyin: f64 = self
            .pinyin
            .get(&pinyin::letters(word))
            .into_iter()
            .flatten()
            .map(|&id| self.words[id as usize].reading_frequency)
            .sum();
        lexicon::lexicon().share(word) > pinyin / self.reading_total.max(1.0)
    }
}

pub(super) fn query_text(text: &str) -> String {
    tidy_chinese(fold_width(text).trim().trim_start_matches('#').trim())
}

// Folding full-width ？ would turn on wildcard syntax.
fn fold_width(text: &str) -> String {
    text.chars()
        .map(|c| {
            if matches!(c, '‘' | '’') {
                return '\'';
            }
            let mut folded = Vec::new();
            unicode_normalization::char::decompose_compatible(c, |d| folded.push(d));
            match folded[..] {
                [d] if d.is_ascii_alphanumeric() || d == ' ' || d == '\'' => d,
                _ => c,
            }
        })
        .collect()
}

fn tidy_chinese(text: &str) -> String {
    if !text.chars().any(is_hanzi) {
        return text.to_string();
    }
    let text = text.trim_matches(|c: char| !c.is_alphanumeric() && !is_wildcard(c));
    let mut tidy = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        let joined = tidy.chars().next_back().is_some_and(is_hanzi)
            && word.chars().next().is_some_and(is_hanzi);
        if !tidy.is_empty() && !joined {
            tidy.push(' ');
        }
        tidy.push_str(word);
    }
    tidy
}
