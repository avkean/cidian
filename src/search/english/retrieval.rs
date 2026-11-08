use super::super::{Keyed, MatchKind, Ranked, RankedCache, RankedKey, Relevance, SearchIndex};
use super::ranking::{EnglishPart, Query, Wanted, better};
use super::{
    fold_english, has_role, initials, joined, normalize_folded, without_generic_possessive,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

#[derive(Clone, Copy)]
enum Place {
    Anywhere,
    Head,
    Lead,
}

pub(in crate::search) const RANKED_CANDIDATES: usize = 1000;
pub(in crate::search) const RANKING_WORK: usize = 60_000;

#[derive(Clone, Eq, Hash, PartialEq)]
pub(in crate::search) struct EnglishQuery {
    written: Lookup,
    joined: Option<Lookup>,
    mixed: bool,
    limit: usize,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct Lookup {
    query: Query,
    tokens: Vec<Wanted>,
    capitals: bool,
    exact: bool,
}

struct RankingBudget {
    remaining: usize,
    exhausted: bool,
}

impl RankingBudget {
    fn take(&mut self, cost: usize) -> bool {
        if let Some(remaining) = self.remaining.checked_sub(cost) {
            self.remaining = remaining;
            true
        } else {
            self.exhausted = true;
            false
        }
    }
}

// bounds kept glosses even if someone searches every word
pub(in crate::search) const MEANINGS_CACHE_CAPACITY: usize = 2048;

#[derive(Default)]
pub(in crate::search) struct MeaningsCache {
    parts: HashMap<u32, Arc<[EnglishPart]>>,
    order: VecDeque<u32>,
}

impl MeaningsCache {
    fn insert(&mut self, id: u32, parts: Arc<[EnglishPart]>) {
        if self.parts.contains_key(&id) {
            return;
        }
        if self.parts.len() == MEANINGS_CACHE_CAPACITY {
            self.parts.remove(&self.order.pop_front().unwrap());
        }
        self.order.push_back(id);
        self.parts.insert(id, parts);
    }
}

impl SearchIndex {
    pub(in crate::search) fn english_matches(
        &self,
        text: &str,
        mixed: bool,
    ) -> Vec<(Relevance, u32)> {
        self.english_with_budget(text, mixed, RANKING_WORK).0
    }

    pub(in crate::search) fn english_with_budget(
        &self,
        text: &str,
        mixed: bool,
        limit: usize,
    ) -> (Vec<(Relevance, u32)>, bool) {
        let query = self.english_query(text, mixed, limit);
        let ranked = RankedCache::get_or_compute(
            &self.ranked,
            RankedKey::English(Box::new(query.clone())),
            || self.rank_english(&query),
        );
        (ranked.matches.clone(), ranked.truncated)
    }

    fn english_query(&self, text: &str, mixed: bool, limit: usize) -> EnglishQuery {
        let joined = joined(text).map(|mut joined| {
            // Dotted letters can name capitals ("u.s."), or a lowercase word ("p.m.").
            let letters = joined.trim_end_matches('.');
            if initials(text).eq([letters.to_string()])
                && !self.initials.contains(letters)
                && self.is_abbreviation(&letters.to_ascii_uppercase())
            {
                joined = letters.to_ascii_uppercase();
            }
            self.english_lookup(&joined, true)
        });
        EnglishQuery {
            written: self.english_lookup(text, false),
            joined,
            mixed,
            limit,
        }
    }

    // The cache key is the complete input to this computation. Typed text is only
    // needed afterwards, for corrections, fallbacks and the displayed query.
    fn rank_english(&self, query: &EnglishQuery) -> Ranked {
        let mut budget = RankingBudget {
            remaining: query.limit,
            exhausted: false,
        };
        let mut matches = self.english_matches_as_written(&query.written, query.mixed, &mut budget);
        let Some(joined) = &query.joined else {
            return Ranked {
                matches,
                truncated: budget.exhausted,
                ..Ranked::default()
            };
        };
        let mut found: HashMap<u32, usize> = matches
            .iter()
            .enumerate()
            .map(|(index, (_, id))| (*id, index))
            .collect();
        // just the joined spelling, not its forms ("be" of "am")
        for (mut rank, id) in self.english_matches_as_written(joined, query.mixed, &mut budget) {
            rank.hidden = true;
            match found.get(&id) {
                Some(&index) => {
                    let kept = matches[index].0;
                    if better(rank, kept) {
                        matches[index].0 = rank;
                    }
                    matches[index].0.named |= kept.named || rank.named;
                }
                None => {
                    found.insert(id, matches.len());
                    matches.push((rank, id));
                }
            }
        }
        Ranked {
            matches,
            truncated: budget.exhausted,
            ..Ranked::default()
        }
    }

    fn english_lookup(&self, text: &str, exact: bool) -> Lookup {
        let capitals = self.is_abbreviation(text);
        let (text, _) = without_generic_possessive(text);
        let text = fold_english(&text);
        let query = self
            .query(&normalize_folded(&text))
            .exact_if(capitals)
            .without_forms_if(exact);
        let tokens = if query.words.is_empty() {
            self.query(&text)
                .exact_if(capitals)
                .without_forms_if(exact)
                .words
        } else {
            query
                .without_possessive
                .as_ref()
                .unwrap_or(&query.words)
                .clone()
        };
        Lookup {
            query,
            tokens,
            capitals,
            exact,
        }
    }

    fn english_matches_as_written(
        &self,
        lookup: &Lookup,
        mixed: bool,
        budget: &mut RankingBudget,
    ) -> Vec<(Relevance, u32)> {
        let Lookup {
            query,
            tokens,
            capitals,
            exact,
        } = lookup;
        if mixed && query.words.is_empty() {
            return Vec::new();
        }
        let Some(last) = tokens.last() else {
            return Vec::new();
        };
        // Charge each candidate for every lookup token, including repetitions used in phrase ranking.
        let cost = tokens.len();
        // only a whole meaning names a lone stop word whose forms are stop words too (`uses`), and a whole
        // meaning starts with it or follows an adverb, so its lead list holds every word that can name it; the
        // full scan is only for when none does; the joined-spelling pass skips it, and rank_tail adds the marked
        // and supported words itself
        if !mixed
            && !*exact
            && !*capitals
            && tokens.len() == 1
            && query.without_possessive.is_none()
            && std::iter::once(&last.word)
                .chain(last.forms.iter().map(|(form, _)| form))
                .all(|form| has_role(form, "stop"))
        {
            let ids = self.inflected_ids(last, Place::Lead);
            let rows: Vec<(Relevance, u32)> = ids
                .into_iter()
                .take_while(|_| budget.take(cost))
                .map(|id| (self.english_rank(id, query, true), id))
                .collect();
            if rows.iter().any(|(rank, _)| rank.named) {
                return rows;
            }
        }
        let place = if mixed && tokens.len() == 1 {
            Place::Head
        } else {
            Place::Anywhere
        };
        let exact = self.inflected_ids(last, place);
        let mut candidates = exact.clone();
        if !mixed {
            for (word, ids) in self.english.range(last.word.clone()..) {
                if !word.starts_with(last.word.as_str()) {
                    break;
                }
                candidates.extend(
                    ids.all
                        .iter()
                        .filter(|id| ids.joined.binary_search(id).is_err()),
                );
            }
            candidates.sort_unstable();
            candidates.dedup();
        }
        let mut seen = HashSet::new();
        for token in tokens[..tokens.len() - 1]
            .iter()
            .filter(|token| seen.insert(&token.word))
        {
            let ids = self.inflected_ids(token, Place::Anywhere);
            candidates.retain(|id| ids.binary_search(id).is_ok());
        }
        // prefix-only words can only match part of a word, so their meanings need no preparing
        let (whole, partial): (Vec<u32>, Vec<u32>) = candidates
            .into_iter()
            .partition(|id| exact.binary_search(id).is_ok());
        let part_of_word = Relevance::new(MatchKind::Substring);
        whole
            .into_iter()
            .take_while(|_| budget.take(cost))
            .map(|id| {
                let mut rank = self.english_rank(id, query, true);
                rank.named |= tokens.len() > 1;
                (rank, id)
            })
            .chain(partial.into_iter().map(|id| (part_of_word, id)))
            // dropping a personal determiner only finds generic slots, other all-token hits aren't matches
            .filter(|(rank, _)| {
                query.without_possessive.is_none() || rank.kind < MatchKind::Substring
            })
            .collect()
    }

    pub(in crate::search) fn keep_best_parts(
        &self,
        text: &str,
        matches: &mut Vec<(Relevance, u32)>,
        tail: Option<&super::usual::LexicalQuery<'_>>,
    ) -> bool {
        let marked = tail
            .and_then(|query| query.marks)
            .map_or(&[][..], |(_, marked)| marked);
        let is_part = |&(rank, id): &(Relevance, u32)| {
            rank.kind == MatchKind::Substring && !marked.iter().any(|&(_, mark)| mark == id)
        };
        if matches.iter().filter(|item| is_part(item)).count() <= RANKED_CANDIDATES {
            return false;
        }
        let (parts, rest): (Vec<_>, Vec<_>) = matches.drain(..).partition(is_part);
        matches.extend(rest);
        let mut parts: Vec<(f64, Keyed)> = parts
            .into_iter()
            .map(|(rank, id)| {
                let score = tail.map_or(0.0, |query| {
                    let bits = super::tail::support_bits(&query.support, id);
                    super::tail::score(&self.tail_features(text, rank, id, bits))
                });
                (score, self.english_key(rank, id))
            })
            .collect();
        parts.select_nth_unstable_by(RANKED_CANDIDATES, |a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| self.english_order(&a.1, &b.1))
        });
        parts.truncate(RANKED_CANDIDATES);
        matches.extend(parts.into_iter().map(|(_, (_, rank, id))| (rank, id)));
        true
    }

    pub(super) fn with_meanings<T>(
        &self,
        id: u32,
        keep: bool,
        use_parts: impl FnOnce(&[EnglishPart]) -> T,
    ) -> T {
        let cached = self.meanings.lock().unwrap().parts.get(&id).cloned();
        let parts = cached.unwrap_or_else(|| {
            let parts: Arc<[EnglishPart]> =
                EnglishPart::prepare(&self.words[id as usize], &self.replaced).into();
            if keep {
                self.meanings.lock().unwrap().insert(id, parts.clone());
            }
            parts
        });
        use_parts(&parts)
    }

    fn inflected_ids(&self, wanted: &Wanted, place: Place) -> Vec<u32> {
        let mut ids = Vec::new();
        for form in std::iter::once(&wanted.word).chain(wanted.forms.iter().map(|(form, _)| form)) {
            let Some(posting) = self.english.get(form) else {
                continue;
            };
            let having = |test: fn(&EnglishPart, &str) -> bool| -> Vec<u32> {
                posting
                    .all
                    .iter()
                    .copied()
                    .filter(|&id| {
                        self.with_meanings(id, true, |parts| {
                            parts.iter().any(|part| test(part, form))
                        })
                    })
                    .collect()
            };
            ids.extend(match place {
                Place::Anywhere => &posting.all,
                Place::Head => posting
                    .heads
                    .get_or_init(|| having(|part, form| part.heads().any(|word| word == form))),
                Place::Lead => posting
                    .leads
                    .get_or_init(|| having(|part, form| part.leads().any(|word| word == form))),
            });
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub(super) fn head_ids(&self, query: &Query) -> Vec<u32> {
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        for words in std::iter::once(&query.words).chain(&query.without_possessive) {
            for word in words
                .first()
                .into_iter()
                .chain(words.last())
                .filter(|word| seen.insert(&word.word))
            {
                ids.extend(self.inflected_ids(word, Place::Lead));
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub(super) fn gloss_ids(&self, query: &Query) -> Vec<u32> {
        let words = query.without_possessive.as_ref().unwrap_or(&query.words);
        let mut found: Option<Vec<u32>> = None;
        let mut seen = HashSet::new();
        for word in words.iter().filter(|word| seen.insert(&word.word)) {
            let ids = self.inflected_ids(word, Place::Anywhere);
            if let Some(found) = &mut found {
                found.retain(|id| ids.binary_search(id).is_ok());
            } else {
                found = Some(ids);
            }
        }
        found.unwrap_or_default()
    }
}
