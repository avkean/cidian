mod chinese;
mod english;
mod index;
mod lexicon;
mod model;
mod pinyin;
mod query;
mod results;
mod spelling;
mod tail;
mod usual;

use english::{FullText, MeaningsCache, Tier, fold_english, rank_order, tokens};
pub use model::*;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::File;
use std::sync::{Arc, Mutex, OnceLock, Weak};

const NEWS_CORROBORATION_FLOOR: f64 = 50.0;

#[derive(Default)]
enum Line {
    #[default]
    None,
    Pinyin(Vec<u32>),
    Capitals,
}

pub struct SearchIndex {
    pub words: Vec<Word>,
    usual: HashMap<String, Vec<(u8, u32)>>,
    support: tail::SupportIndex,
    headwords: BTreeMap<String, Vec<u32>>,
    loose: HashMap<String, Vec<u32>>,
    longest_headword: usize,
    log_total: f64,
    pinyin: BTreeMap<String, Vec<u32>>,
    syllables: HashSet<String>,
    reading_total: f64,
    unsupported_characters: HashSet<u32>,
    english: BTreeMap<String, EnglishPostings>,
    characters: HashMap<char, Vec<u32>>,
    abbreviations: HashMap<String, Vec<u32>>,
    initials: HashSet<String>,
    initialisms: HashMap<&'static str, Vec<(u32, Lead)>>,
    speller: spelling::Speller,
    meanings: Mutex<MeaningsCache>,
    ranked: Mutex<RankedCache>,
    fulltext: OnceLock<FullText>,
    lines: Option<(File, Vec<(u64, u32)>)>,
    examples_failed: std::sync::atomic::AtomicBool,
    /// a rewritten meaning's old text by word and sense: it still finds and ranks the meaning but is
    /// never shown, and few meanings have one, so it's kept apart from the senses
    replaced: HashMap<(u32, usize), Box<str>>,
}

#[derive(Default)]
struct Ranked {
    matches: Vec<(Relevance, u32)>,
    mode: String,
    text: String,
    truncated: bool,
    corrected: Option<String>,
    related: Option<String>,
    segments: Vec<SearchSegment>,
    unfound: Vec<String>,
    line: Line,
}

#[derive(Clone, Eq, Hash, PartialEq)]
enum RankedKey {
    Search(String, String, bool),
    English(Box<english::EnglishQuery>),
}

#[derive(Default)]
struct RankedCache {
    lists: VecDeque<(RankedKey, Arc<Ranked>)>,
    rows: usize,
    pending: HashMap<RankedKey, Weak<OnceLock<Arc<Ranked>>>>,
}

const RANKED_LISTS: usize = 64;
const RANKED_ROWS: usize = 65_536;

impl RankedCache {
    fn get_or_compute(
        cache: &Mutex<Self>,
        key: RankedKey,
        compute: impl FnOnce() -> Ranked,
    ) -> Arc<Ranked> {
        let pending = {
            let mut cache = cache.lock().unwrap();
            if let Some(ranked) = cache.get(&key) {
                return ranked;
            }
            cache
                .pending
                .retain(|_, pending| pending.strong_count() > 0);
            match cache.pending.get(&key).and_then(Weak::upgrade) {
                Some(pending) => pending,
                None => {
                    let pending = Arc::new(OnceLock::new());
                    cache.pending.insert(key.clone(), Arc::downgrade(&pending));
                    pending
                }
            }
        };
        let ranked = pending
            .get_or_init(|| {
                let mut ranked = compute();
                ranked.matches.shrink_to_fit();
                Arc::new(ranked)
            })
            .clone();
        let mut cache = cache.lock().unwrap();
        cache.insert(key.clone(), ranked.clone());
        if cache
            .pending
            .get(&key)
            .is_some_and(|held| held.ptr_eq(&Arc::downgrade(&pending)))
        {
            cache.pending.remove(&key);
        }
        ranked
    }

    fn get(&mut self, key: &RankedKey) -> Option<Arc<Ranked>> {
        let at = self.lists.iter().position(|(held, _)| held == key)?;
        let entry = self.lists.remove(at)?;
        let ranked = entry.1.clone();
        self.lists.push_back(entry);
        Some(ranked)
    }

    fn insert(&mut self, key: RankedKey, ranked: Arc<Ranked>) {
        if ranked.matches.len() > RANKED_ROWS || self.lists.iter().any(|(held, _)| *held == key) {
            return;
        }
        self.rows += ranked.matches.len();
        self.lists.push_back((key, ranked));
        while self.lists.len() > RANKED_LISTS || self.rows > RANKED_ROWS {
            if let Some((_, old)) = self.lists.pop_front() {
                self.rows -= old.matches.len();
            }
        }
    }
}

type Keyed = ((Tier, i32), Relevance, u32);

struct EnglishPostings {
    all: Vec<u32>,
    joined: Vec<u32>,
    // cached per term so broad words don't rescan every meaning per keystroke
    heads: OnceLock<Vec<u32>>,
    leads: OnceLock<Vec<u32>>,
}

impl SearchIndex {
    fn ordering(&self, a: u32, b: u32) -> std::cmp::Ordering {
        let (x, y) = (&self.words[a as usize], &self.words[b as usize]);
        y.reading_frequency
            .total_cmp(&x.reading_frequency)
            .then_with(|| {
                y.reading_corpus_frequency
                    .total_cmp(&x.reading_corpus_frequency)
            })
            .then_with(|| y.frequency.total_cmp(&x.frequency))
            .then_with(|| {
                x.simplified
                    .chars()
                    .count()
                    .cmp(&y.simplified.chars().count())
            })
            .then_with(|| x.pinyin.cmp(&y.pinyin))
            .then_with(|| x.simplified.cmp(&y.simplified))
            .then_with(|| a.cmp(&b))
    }

    fn sort(&self, ids: &mut [u32]) {
        ids.sort_unstable_by(|&a, &b| self.ordering(a, b));
    }

    fn order_characters<T: Copy>(&self, items: &mut [T], id: impl Fn(&T) -> u32) {
        let single = |item: &T| self.words[id(item) as usize].simplified.chars().count() == 1;
        let mut characters: Vec<_> = items.iter().copied().filter(single).collect();
        characters.sort_by_key(|item| self.unsupported_characters.contains(&id(item)));
        let mut characters = characters.into_iter();
        for item in items.iter_mut().filter(|item| single(item)) {
            *item = characters.next().unwrap();
        }
    }

    fn english_tie(&self, rank: Relevance, id: u32) -> (u64, bool, (bool, usize)) {
        if !(MatchKind::WholeGloss..MatchKind::Pinyin).contains(&rank.kind) {
            return (0, true, (false, 0));
        }
        let word = &self.words[id as usize];
        (word.corpus_frequency, !rank.regional, word.attestation())
    }

    fn rank(&self, matches: &mut Vec<(Relevance, u32)>, english: bool) {
        let mut keyed: Vec<_> = matches
            .iter()
            .map(|&(rank, id)| {
                let key = if english {
                    self.english_key(rank, id).0
                } else {
                    (Tier::Other(rank.kind), 0)
                };
                (key, rank, id)
            })
            .collect();
        keyed.sort_unstable_by_key(|&(key, rank, id)| (id, key, rank));
        keyed.dedup_by(|later, kept| {
            let same = later.2 == kept.2;
            kept.1.named |= same && later.1.named;
            same
        });
        keyed.sort_unstable_by(|a, b| {
            if english {
                self.english_order(a, b)
            } else {
                a.0.cmp(&b.0).then_with(|| self.ordering(a.2, b.2))
            }
        });
        matches.clear();
        matches.extend(keyed.into_iter().map(|(_, rank, id)| (rank, id)));
    }

    fn english_key(&self, rank: Relevance, id: u32) -> Keyed {
        ((rank_order(rank).0, self.blend(rank, id)), rank, id)
    }

    fn english_order(&self, a: &Keyed, b: &Keyed) -> std::cmp::Ordering {
        a.0.cmp(&b.0)
            .then_with(|| {
                let (x, y) = (&self.words[a.2 as usize], &self.words[b.2 as usize]);
                // keep jieba's scale; counts from another corpus only break ties
                y.frequency
                    .total_cmp(&x.frequency)
                    .then_with(|| self.english_tie(b.1, b.2).cmp(&self.english_tie(a.1, a.2)))
            })
            .then_with(|| self.ordering(a.2, b.2))
    }
}

pub fn is_hanzi(c: char) -> bool {
    matches!(c as u32, 0x3007 | 0x3400..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
}
