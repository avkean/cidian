use super::super::{Keyed, MatchKind, Relevance, SearchIndex, is_hanzi};
use super::retrieval::RANKED_CANDIDATES;
use super::{fold_english, normalize_folded};
use std::collections::{HashMap, HashSet};

const SETTLED_AT_ONCE: usize = 100;
const SCORED_CANDIDATES: usize = 10_000;

struct RankingBudget {
    remaining: usize,
    exhausted: bool,
}

impl RankingBudget {
    fn take(&mut self) -> bool {
        if self.remaining == 0 {
            self.exhausted = true;
            return false;
        }
        self.remaining -= 1;
        true
    }
}

pub(in crate::search) fn fulltext_query(text: &str) -> String {
    fold_english(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(in crate::search) fn distinct<'a>(
    terms: impl Iterator<Item = &'a str>,
) -> impl Iterator<Item = &'a str> {
    let mut seen = HashSet::new();
    terms.filter(move |term| seen.insert(*term))
}

pub(in crate::search) struct FullText {
    text: String,
    starts: Vec<usize>,
    examples: Vec<usize>,
    terms: String,
    term_starts: Vec<usize>,
    postings: Vec<u32>,
    posting_starts: Vec<usize>,
}

pub(in crate::search) fn terms(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric() || is_hanzi(c))
        .filter(|term| !term.is_empty())
}

impl FullText {
    fn new(index: &SearchIndex) -> Self {
        let words = &index.words;
        let mut corpus = Self {
            text: String::new(),
            starts: Vec::with_capacity(words.len()),
            examples: Vec::with_capacity(words.len()),
            terms: String::new(),
            term_starts: Vec::new(),
            postings: Vec::new(),
            posting_starts: Vec::new(),
        };
        for word in words {
            corpus.starts.push(corpus.text.len());
            for sense in word.searchable_senses().map(|(_, sense)| sense) {
                corpus.push(&fold_english(&sense.text));
            }
            corpus.examples.push(corpus.text.len());
            for example in index.examples(word).iter() {
                for part in [&example.chinese, &example.pinyin, &example.english] {
                    corpus.push(&fold_english(part));
                }
            }
            for form in std::iter::once(&word.simplified).chain(&word.traditional) {
                corpus.push(form);
            }
            corpus.text.push('\0');
        }
        corpus.text.shrink_to_fit();
        let mut numbers: HashMap<&str, u32> = HashMap::new();
        let (mut names, mut counts, mut last) = (Vec::new(), Vec::new(), Vec::new());
        let mut found = Vec::new();
        let mut ends = Vec::with_capacity(words.len());
        for id in 0..words.len() as u32 {
            for term in terms(corpus.record(id)) {
                let number = *numbers.entry(term).or_insert_with(|| {
                    names.push(term);
                    counts.push(0);
                    last.push(u32::MAX);
                    names.len() as u32 - 1
                });
                if last[number as usize] != id {
                    last[number as usize] = id;
                    counts[number as usize] += 1;
                    found.push(number);
                }
            }
            ends.push(found.len());
        }
        let mut posting_starts = Vec::with_capacity(counts.len() + 1);
        let mut total = 0;
        for count in counts {
            posting_starts.push(total);
            total += count;
        }
        posting_starts.push(total);
        let mut postings = vec![0; total];
        let mut next = posting_starts.clone();
        let mut start = 0;
        for (id, end) in ends.into_iter().enumerate() {
            for &number in &found[start..end] {
                postings[next[number as usize]] = id as u32;
                next[number as usize] += 1;
            }
            start = end;
        }
        let mut terms = String::new();
        let mut term_starts = Vec::with_capacity(names.len());
        for name in names {
            term_starts.push(terms.len());
            terms.push_str(name);
            terms.push('\n');
        }
        terms.shrink_to_fit();
        Self {
            terms,
            term_starts,
            postings,
            posting_starts,
            ..corpus
        }
    }

    fn push(&mut self, line: &str) {
        self.text.push_str(line);
        self.text.push('\n');
    }

    fn record(&self, id: u32) -> &str {
        let end = self
            .starts
            .get(id as usize + 1)
            .copied()
            .unwrap_or(self.text.len());
        &self.text[self.starts[id as usize]..end]
    }

    fn meanings(&self, id: u32) -> &str {
        &self.text[self.starts[id as usize]..self.examples[id as usize]]
    }

    fn find(&self, needle: &str) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .text
            .match_indices(needle)
            .map(|(offset, _)| (self.starts.partition_point(|&start| start <= offset) - 1) as u32)
            .collect();
        ids.dedup();
        ids
    }

    fn postings_with<'a>(&'a self, part: &'a str) -> impl Iterator<Item = &'a [u32]> + 'a {
        let mut last = None;
        self.terms
            .match_indices(part)
            .filter_map(move |(offset, _)| {
                let term = self.term_starts.partition_point(|&start| start <= offset) - 1;
                (last.replace(term) != Some(term)).then(|| {
                    &self.postings[self.posting_starts[term]..self.posting_starts[term + 1]]
                })
            })
    }

    fn find_rarest(&self, tokens: &[&str]) -> Option<Vec<u32>> {
        let rarest = distinct(tokens.iter().flat_map(|token| terms(token)))
            .min_by_key(|part| self.postings_with(part).map(<[u32]>::len).sum::<usize>())?;
        let mut ids: Vec<u32> = self.postings_with(rarest).flatten().copied().collect();
        ids.sort_unstable();
        ids.dedup();
        Some(ids)
    }

    fn kind(&self, id: u32, text: &str, tokens: &[&str]) -> MatchKind {
        let meanings = self.meanings(id);
        if meanings.contains(text) {
            MatchKind::Substring
        } else if tokens.iter().all(|token| meanings.contains(token)) {
            MatchKind::InMeanings
        } else {
            MatchKind::InExamples
        }
    }
}

impl SearchIndex {
    pub fn prepare_fulltext(&self) {
        self.fulltext();
    }

    fn fulltext(&self) -> &FullText {
        self.fulltext.get_or_init(|| FullText::new(self))
    }

    pub(in crate::search) fn fulltext_matches(&self, text: &str) -> (Vec<(Relevance, u32)>, bool) {
        self.fulltext_with_budget(text, SCORED_CANDIDATES)
    }

    pub(in crate::search) fn fulltext_with_budget(
        &self,
        text: &str,
        limit: usize,
    ) -> (Vec<(Relevance, u32)>, bool) {
        let corpus = self.fulltext();
        let text = fulltext_query(text);
        let tokens: Vec<&str> = distinct(text.split_whitespace()).collect();
        let Some(longest) = tokens.iter().max_by_key(|token| token.len()) else {
            return (Vec::new(), false);
        };
        // one letter or digit is in nearly every record
        if longest.len() < 2 {
            return (Vec::new(), false);
        }
        // every phrase match is also an all-token match: take the rarest term's records (or scan for the
        // longest chinese token), then check every token
        let mut ids = corpus
            .find_rarest(&tokens)
            .unwrap_or_else(|| corpus.find(longest));
        ids.retain(|&id| tokens.iter().all(|token| corpus.record(id).contains(token)));
        let query = self.query(&normalize_folded(&text));
        let mut budget = RankingBudget {
            remaining: limit,
            exhausted: false,
        };
        let mut rank = |id| {
            if !budget.take() {
                return None;
            }
            let rank = self.english_rank(id, &query, false);
            Some(if rank.kind < MatchKind::Substring {
                rank
            } else {
                Relevance::new(corpus.kind(id, &text, &tokens))
            })
        };
        // only meanings with every query word can match as glosses, and only those with it at a head as a
        // whole gloss or head, so rank those; the rest can at best mention it
        let truncated = ids.len() > RANKED_CANDIDATES;
        let glossed = self.gloss_ids(&query);
        let headed = if truncated {
            self.head_ids(&query)
        } else {
            glossed.clone()
        };
        let (mut ranked, mut bounded): (Vec<Keyed>, Vec<Keyed>) = (Vec::new(), Vec::new());
        for id in ids {
            if glossed.binary_search(&id).is_err() {
                bounded.push(self.english_key(Relevance::new(corpus.kind(id, &text, &tokens)), id));
            } else if headed.binary_search(&id).is_ok() {
                if let Some(rank) = rank(id) {
                    ranked.push(self.english_key(rank, id));
                }
            } else {
                bounded.push(self.english_key(Relevance::new(MatchKind::Mention), id));
            }
        }
        let mut settle = |item: Keyed| {
            if item.1.kind == MatchKind::Mention {
                rank(item.2).map(|rank| self.english_key(rank, item.2))
            } else {
                Some(item)
            }
        };
        if truncated {
            // common words ("the") are in most records: rank those with the best possible order first, then a
            // page at a time while any could still beat the last kept
            let order = |a: &Keyed, b: &Keyed| self.english_order(a, b);
            let keep_best = |ranked: &mut Vec<Keyed>| {
                if ranked.len() < RANKED_CANDIDATES {
                    return None;
                }
                ranked.select_nth_unstable_by(RANKED_CANDIDATES - 1, order);
                ranked.truncate(RANKED_CANDIDATES);
                Some(ranked[RANKED_CANDIDATES - 1])
            };
            let take = RANKED_CANDIDATES.min(bounded.len());
            if take < bounded.len() {
                bounded.select_nth_unstable_by(take, order);
            }
            let rest = bounded.split_off(take);
            ranked.extend(bounded.into_iter().filter_map(&mut settle));
            let mut last = keep_best(&mut ranked);
            let mut rest: Vec<Keyed> = rest
                .into_iter()
                .filter(|item| last.is_none_or(|last| order(item, &last).is_lt()))
                .collect();
            rest.sort_unstable_by(order);
            for page in rest.chunks(SETTLED_AT_ONCE) {
                if last.is_some_and(|last| !order(&page[0], &last).is_lt()) {
                    break;
                }
                ranked.extend(page.iter().copied().filter_map(&mut settle));
                last = keep_best(&mut ranked);
            }
        } else {
            ranked.extend(bounded.into_iter().filter_map(&mut settle));
        }
        let matches = ranked.into_iter().map(|(_, rank, id)| (rank, id)).collect();
        (matches, truncated || budget.exhausted)
    }
}
