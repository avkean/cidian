use super::english::fold_english;
use super::pinyin::has_wildcard;
use super::{
    Line, MatchKind, Ranked, Reading, Relevance, SearchIndex, SearchQuery, SearchResponse,
    SearchSegment, Summary, Word, english, is_hanzi, lexicon, pinyin,
};
use std::collections::{HashMap, HashSet};

// three separate entries back an abbreviation, one or two can be chance
const ABBREVIATION_ENTRY_FLOOR: usize = 3;
const RELATED_LIMIT: usize = 40;
const SEGMENT_RESULTS: usize = 3;
const PHRASE_GROUPS: usize = 3;
/// words tried for them, so a long phrase can't fan out into dozens of searches
const PHRASE_TRIES: usize = 9;
/// each part is a whole search, so a long phrase can't fan out into dozens
const SEGMENT_SEARCHES: usize = 3;

impl SearchIndex {
    pub fn readings(&self, word: &Word) -> Vec<Reading> {
        let mut ids: Vec<u32> = self
            .by_headword(&word.simplified)
            .iter()
            .copied()
            .filter(|&id| self.words[id as usize].simplified == word.simplified)
            .collect();
        self.sort(&mut ids);
        ids.into_iter()
            .map(|id| {
                let word = &self.words[id as usize];
                Reading {
                    id,
                    pinyin: word.pinyin.clone(),
                    written: word.written.clone(),
                    word_starts: word.word_starts,
                }
            })
            .collect()
    }

    pub fn exact_summaries(&self, headword: &str) -> Vec<Summary> {
        let mut ids = self.by_headword(headword).to_vec();
        self.sort(&mut ids);
        self.summaries(ids)
    }

    fn summaries(&self, ids: impl IntoIterator<Item = u32>) -> Vec<Summary> {
        ids.into_iter()
            .map(|id| self.words[id as usize].summary())
            .collect()
    }

    pub fn related(&self, word: &Word, offset: usize) -> (Vec<Summary>, usize) {
        let rarest = word
            .simplified
            .chars()
            .filter_map(|c| self.characters.get(&c))
            .min_by_key(|ids| ids.len());
        let Some(candidates) = rarest else {
            return (Vec::new(), 0);
        };
        let traditional = word.traditional_form();
        let mut matches: Vec<(Relevance, u32)> = candidates
            .iter()
            .filter_map(|&id| {
                let candidate = &self.words[id as usize];
                if candidate.simplified == word.simplified {
                    return None;
                }
                let candidate_traditional = candidate.traditional_form();
                if candidate.simplified.starts_with(&word.simplified)
                    || candidate_traditional.starts_with(traditional)
                {
                    Some((Relevance::new(MatchKind::Prefix), id))
                } else if candidate.simplified.contains(&word.simplified)
                    || candidate_traditional.contains(traditional)
                {
                    Some((Relevance::new(MatchKind::Contains), id))
                } else {
                    None
                }
            })
            .collect();
        self.rank(&mut matches, false);
        let total = matches.len();
        let shown = self.summaries(
            matches
                .into_iter()
                .skip(offset)
                .take(RELATED_LIMIT)
                .map(|(_, id)| id),
        );
        (shown, total)
    }

    pub(super) fn page(&self, query: &SearchQuery, ranked: &Ranked) -> SearchResponse {
        let text = &ranked.text;
        let mode = ranked.mode.as_str();
        let mut response = SearchResponse {
            query: query.q.clone(),
            mode: ranked.mode.clone(),
            total: ranked.matches.len(),
            results: Vec::new(),
            exact: Vec::new(),
            corrected: ranked.corrected.clone(),
            related: ranked.related.clone(),
            selected: self
                .by_headword(&query.word)
                .iter()
                .copied()
                .find(|&id| self.words[id as usize].pinyin == query.reading),
            segments: ranked.segments.clone(),
            unfound: ranked.unfound.clone(),
            pinyin: Vec::new(),
            english: Vec::new(),
            capitals: None,
            truncated: ranked.truncated,
        };
        if text.is_empty() {
            return response;
        }
        if query.offset == 0 {
            match &ranked.line {
                Line::None => {}
                Line::Pinyin(ids) => response.pinyin = self.summaries(ids.iter().copied()),
                Line::Capitals => {
                    if let Some((capitals, ids)) = self.capitals_line(text) {
                        response.english = self.summaries(ids);
                        response.capitals = Some(capitals);
                    }
                }
            }
        }
        let exact_text = (mode == "fulltext").then(|| super::query::query_text(&query.q));
        let exact_ids: HashSet<u32> = self
            .exact(exact_text.as_deref().unwrap_or(text))
            .iter()
            .copied()
            .collect();
        let limit = query.page_size();
        let wanted = (matches!(mode, "english" | "mixed") && !text.chars().any(is_hanzi))
            .then(|| self.ranking_query(text, self.is_abbreviation(text)));
        for &(rank, id) in ranked.matches.iter().skip(query.offset).take(limit) {
            if exact_ids.contains(&id) {
                response.exact.push(id);
            }
            let lead = match &wanted {
                Some(wanted) if rank.kind < MatchKind::Pinyin => {
                    self.row_lead(rank, id, wanted, true)
                }
                _ => rank.lead,
            };
            response
                .results
                .push(self.words[id as usize].summary_leading(lead));
        }
        response
    }

    pub(super) fn capitals_line(&self, text: &str) -> Option<(String, Vec<u32>)> {
        let capitals = text.to_ascii_uppercase();
        if !self.is_abbreviation(&capitals) {
            return None;
        }
        let folded = fold_english(&capitals);
        let entries = self
            .abbreviations
            .get(folded.trim_matches(|c: char| !c.is_ascii_alphanumeric()))?;
        if entries.len() < ABBREVIATION_ENTRY_FLOOR {
            let news: f64 = entries
                .iter()
                .map(|&id| self.words[id as usize].reading_corpus_frequency)
                .sum();
            let pinyin_news: f64 = self
                .pinyin
                .get(&pinyin::letters(text))
                .into_iter()
                .flatten()
                .map(|&id| self.words[id as usize].reading_corpus_frequency)
                .sum();
            if news <= pinyin_news {
                return None;
            }
        }
        let mut matches: Vec<_> = self
            .english_matches(&capitals, true)
            .into_iter()
            .filter(|(rank, _)| rank.shown_whole())
            .collect();
        if matches.is_empty() {
            return None;
        }
        let whole: HashSet<u32> = matches.iter().map(|(_, id)| *id).collect();
        // ranks are per word, so ranking just these keeps the full search order
        self.rank(&mut matches, true);
        self.rank_tail(&capitals, &mut matches, &self.lexical_query(&capitals));
        let mut seen = HashSet::new();
        let ids = matches
            .into_iter()
            .map(|(_, id)| id)
            .filter(|id| {
                whole.contains(id) && seen.insert(self.words[*id as usize].simplified.as_str())
            })
            .take(3)
            .collect();
        Some((capitals, ids))
    }

    pub(super) fn split_phrase(
        &self,
        text: &str,
        matches: &mut Vec<(Relevance, u32)>,
        groups: &mut Vec<SearchSegment>,
        unfound: &mut Vec<String>,
    ) {
        if text.chars().any(is_hanzi) || has_wildcard(text) {
            return;
        }
        let lexicon = lexicon::lexicon();
        let lower = text.to_lowercase();
        let tokens: Vec<&str> = lower
            .split_whitespace()
            .map(|token| token.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|token| !token.is_empty())
            .collect();
        let mut words: Vec<&str> = Vec::new();
        for &token in &tokens {
            if token.chars().count() > 1
                && !token.contains(['\'', '’'])
                && !english::has_role(token, "stop")
                && !words.contains(&token)
            {
                words.push(token);
            }
        }
        if words.len() < 2 {
            return;
        }
        let first = tokens
            .iter()
            .position(|token| words.contains(token))
            .unwrap_or(0);
        let before = tokens[first..]
            .iter()
            .position(|token| english::has_role(token, "stop"))
            .map_or(tokens.len(), |at| first + at);
        let noun = |word: &str| {
            lexicon.noun_lemma(word)
                || lexicon
                    .lemmas(word, true)
                    .iter()
                    .any(|lemma| lexicon.noun_lemma(lemma))
        };
        let head = tokens[..before]
            .iter()
            .rfind(|token| words.contains(token) && noun(token))
            .copied()
            .unwrap_or(words[words.len() - 1]);
        let mut others: Vec<&str> = words.iter().copied().filter(|&word| word != head).collect();
        others.sort_by_key(|word| lexicon.count(word));
        let mut seen = HashSet::new();
        for word in std::iter::once(head).chain(others).take(PHRASE_TRIES) {
            if groups.len() == PHRASE_GROUPS {
                break;
            }
            let ids = Self::top_ids(&self.cached_ranked(&SearchQuery {
                q: word.into(),
                mode: "english".into(),
                ..SearchQuery::default()
            }));
            if ids.is_empty() {
                unfound.push(word.into());
                continue;
            }
            let wanted = self.ranking_query(word, false);
            for &id in &ids {
                if seen.insert(id) {
                    let mut rank = Relevance::new(MatchKind::Exact);
                    rank.lead = self.english_rank(id, &wanted, true).lead;
                    matches.push((rank, id));
                }
            }
            groups.push(SearchSegment {
                text: word.into(),
                ids,
            });
        }
        if groups.is_empty() {
            unfound.clear();
        }
    }

    fn top_ids(ranked: &Ranked) -> Vec<u32> {
        ranked
            .matches
            .iter()
            .take(SEGMENT_RESULTS)
            .map(|&(_, id)| id)
            .collect()
    }

    pub(super) fn break_down(
        &self,
        text: &str,
        matches: &mut Vec<(Relevance, u32)>,
        segments: &mut Vec<SearchSegment>,
    ) {
        let pattern = !matches.is_empty() && text.chars().any(|c| c.is_ascii_alphabetic());
        if text.chars().count() < 2 || !text.chars().any(is_hanzi) || has_wildcard(text) || pattern
        {
            return;
        }
        let mut parts = self.split(text, self.longest_headword);
        // letters written apart from the words are a search of their own (中国 food); glued ones are pinyin
        let mut searched: HashMap<String, Vec<u32>> = HashMap::new();
        for (piece, ids) in &mut parts {
            let text = piece.trim().to_lowercase();
            if ids.is_empty()
                && piece.trim() != piece
                && !piece.chars().any(is_hanzi)
                && piece.chars().any(char::is_alphanumeric)
                && (searched.len() < SEGMENT_SEARCHES || searched.contains_key(&text))
            {
                *ids = searched
                    .entry(text.clone())
                    .or_insert_with(|| {
                        Self::top_ids(&self.rank_query(&SearchQuery {
                            q: text,
                            ..SearchQuery::default()
                        }))
                    })
                    .clone();
            }
        }
        if parts.iter().all(|(_, ids)| ids.is_empty()) {
            return;
        }
        matches.clear();
        let mut seen = HashSet::new();
        for (text, ids) in parts {
            matches.extend(
                ids.iter()
                    .filter(|id| seen.insert(**id))
                    .map(|&id| (Relevance::new(MatchKind::Exact), id)),
            );
            segments.push(SearchSegment { text, ids });
        }
    }

    pub(super) fn split(&self, text: &str, longest: usize) -> Vec<(String, Vec<u32>)> {
        let latin = |c: char| c.is_alphanumeric() && !is_hanzi(c);
        let offsets: Vec<usize> = text
            .char_indices()
            .filter(|&(i, c)| !(latin(c) && text[..i].chars().next_back().is_some_and(latin)))
            .map(|(i, _)| i)
            .chain([text.len()])
            .collect();
        let units = offsets.len() - 1;
        let han = |unit: usize| text[offsets[unit]..].starts_with(is_hanzi);
        let mut best = vec![(0.0, 0usize, 0usize, 0usize); units + 1];
        let mut next = vec![0; units];
        let mut known = vec![false; units];
        let better = |a: (f64, usize, usize, usize), b: (f64, usize, usize, usize)| {
            a.0.total_cmp(&b.0)
                .then((b.1, b.2, b.3).cmp(&(a.1, a.2, a.3)))
                .is_gt()
        };
        for start in (0..units).rev() {
            let (score, uncovered, singles, pieces) = best[start + 1];
            best[start] = (
                score - self.log_total,
                uncovered + 1,
                singles + 1,
                pieces + 1,
            );
            next[start] = start + 1;
            for end in start + 1..=units.min(start + longest) {
                let ids = self.by_headword(&text[offsets[start]..offsets[end]]);
                if ids.is_empty() {
                    continue;
                }
                let frequency: f64 = ids
                    .iter()
                    .map(|&id| self.words[id as usize].frequency.max(0.0))
                    .sum();
                let (score, uncovered, singles, pieces) = best[end];
                let candidate = (
                    score + frequency.max(1.0).ln() - self.log_total,
                    uncovered,
                    singles + usize::from(end == start + 1),
                    pieces + 1,
                );
                if better(candidate, best[start]) {
                    best[start] = candidate;
                    next[start] = end;
                    known[start] = true;
                }
            }
        }
        let mut parts: Vec<(String, Vec<u32>)> = Vec::new();
        let mut start = 0;
        while start < units {
            let end = next[start];
            let piece = &text[offsets[start]..offsets[end]];
            let mut ids = Vec::new();
            if known[start] {
                ids = self.by_headword(piece).to_vec();
                self.sort(&mut ids);
            }
            if ids.is_empty()
                && parts.last().is_some_and(|(text, ids)| {
                    ids.is_empty() && text.chars().all(is_hanzi) == han(start)
                })
            {
                parts.last_mut().unwrap().0.push_str(piece);
            } else {
                parts.push((piece.to_string(), ids));
            }
            start = end;
        }
        parts
    }
}
