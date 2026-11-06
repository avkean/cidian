use super::{MatchKind, Relevance, SearchIndex};
use std::collections::HashMap;
use std::io::{self, BufRead};

/// a pairwise logistic ranker fitted once on curated usual-word queries; features in `tail_features` order
const WEIGHTS: [f64; 15] = [
    -0.6884633323536486,
    -1.4951485420060564,
    -0.1675704100919847,
    0.2516581796318651,
    0.20654867757242848,
    0.499242324003012,
    1.810516653750896,
    1.0550721176726026,
    0.48463331898429,
    1.324038611602334,
    1.9058560747792064,
    -0.04974385239051117,
    0.8096792542766751,
    0.5115526376690628,
    -1.3931701848599582,
];

pub(super) fn score(features: &[f64; 15]) -> f64 {
    features.iter().zip(WEIGHTS).map(|(x, w)| x * w).sum()
}

pub(super) fn match_class(rank: Relevance) -> u8 {
    let class = match rank.kind {
        // "sis" is a word before it's a form of "si"
        MatchKind::WholeGloss | MatchKind::LaterGloss
            if rank.form_cost >= super::lexicon::AMBIGUOUS_FORM_COST =>
        {
            1
        }
        MatchKind::Usual | MatchKind::WholeGloss | MatchKind::LaterGloss | MatchKind::Pinyin => 0,
        MatchKind::GlossHead => 1,
        MatchKind::Mention | MatchKind::InMeanings => 2,
        MatchKind::InExamples => 3,
        _ => 4,
    };
    2 * class + rank.caution
}

#[derive(Default)]
pub(super) struct SupportIndex {
    text: Box<str>,
    // query byte offset and first posting, a sentinel ends both
    offsets: Box<[(u32, u32)]>,
    ids: Box<[u32]>,
    bits: Box<[u8]>,
}

impl SupportIndex {
    pub(super) fn read(reader: impl BufRead, words: &[super::Word]) -> io::Result<Self> {
        let keys: HashMap<(&str, &str), u32> = words
            .iter()
            .zip(0..)
            .map(|(word, id)| ((word.simplified.as_str(), word.pinyin.as_str()), id))
            .collect();
        let mut text = String::new();
        let mut offsets: Vec<(u32, u32)> = Vec::new();
        let mut ids = Vec::new();
        let mut bits = Vec::new();
        let mut reader = reader;
        let mut line = String::new();
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid English support TSV");
        let offset = |n| u32::try_from(n).map_err(|_| invalid());
        let mut rows = 0;
        while reader.read_line(&mut line)? != 0 {
            let row = line.trim_end_matches(['\r', '\n']);
            if !row.is_empty() && !row.starts_with('#') {
                rows += 1;
                let mut fields = row.split('\t');
                let query = fields.next().ok_or_else(invalid)?;
                let reading = fields
                    .next()
                    .and_then(|s| s.split_once('|'))
                    .ok_or_else(invalid)?;
                let flags: u8 = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(invalid)?;
                if query.is_empty() || !(1..=7).contains(&flags) || fields.next().is_some() {
                    return Err(invalid());
                }
                let &id = keys.get(&reading).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "English support TSV does not match the dictionary",
                    )
                })?;
                let previous = offsets.last().map(|&(start, _)| &text[start as usize..]);
                if previous.is_some_and(|previous| previous > query) {
                    return Err(invalid());
                }
                if previous != Some(query) {
                    offsets.push((offset(text.len())?, offset(ids.len())?));
                    text.push_str(query);
                }
                ids.push(id);
                bits.push(flags);
            }
            line.clear();
        }
        // an empty file would silently turn source agreement off
        if rows == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "English support TSV has no rows",
            ));
        }
        offsets.push((offset(text.len())?, offset(ids.len())?));
        Ok(Self {
            text: text.into_boxed_str(),
            offsets: offsets.into_boxed_slice(),
            ids: ids.into_boxed_slice(),
            bits: bits.into_boxed_slice(),
        })
    }

    fn get(&self, query: &str) -> impl Iterator<Item = (u32, u8)> + '_ {
        let mut low = 0;
        let mut high = self.offsets.len().saturating_sub(1);
        let mut range = 0..0;
        while low < high {
            let mid = low + (high - low) / 2;
            let (start, first) = self.offsets[mid];
            let (end, last) = self.offsets[mid + 1];
            match self.text[start as usize..end as usize].cmp(query) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => {
                    range = first as usize..last as usize;
                    break;
                }
            }
        }
        self.ids[range.clone()]
            .iter()
            .copied()
            .zip(self.bits[range].iter().copied())
    }
}

const CORE: u8 = 1;
const WIKTIONARY: u8 = 2;
const CC_CEDICT: u8 = 4;

pub(super) fn support_bits(support: &[(u32, u8)], id: u32) -> u8 {
    support
        .binary_search_by_key(&id, |&(id, _)| id)
        .map_or(0, |i| support[i].1)
}

impl SearchIndex {
    pub(super) fn support_for(&self, queries: &[String]) -> Vec<(u32, u8)> {
        let mut result = Vec::new();
        for query in queries {
            let query = query.strip_prefix("to ").unwrap_or(query);
            result.extend(self.support.get(query));
        }
        result.sort_unstable_by_key(|&(id, _)| id);
        result.dedup_by(|later, first| {
            if later.0 != first.0 {
                return false;
            }
            first.1 |= later.1;
            true
        });
        result
    }

    pub(super) fn tail_features(
        &self,
        text: &str,
        rank: Relevance,
        id: u32,
        support: u8,
    ) -> [f64; 15] {
        let word = &self.words[id as usize];
        let match_type = match rank.kind {
            MatchKind::WholeGloss | MatchKind::LaterGloss => 0.0,
            MatchKind::GlossHead => 1.0,
            MatchKind::Mention if !rank.tail_explanation => 2.0,
            _ => 3.0,
        };
        let depth = f64::from(rank.sense_depth).ln_1p();
        let verb_query = text
            .trim_start()
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("to "));
        [
            match_type,
            if rank.core_sense { depth } else { 0.0 },
            if rank.core_sense { 0.0 } else { depth },
            word.reading_frequency.max(0.0).ln_1p(),
            word.reading_corpus_frequency.max(0.0).ln_1p(),
            f64::from(support & CORE != 0),
            f64::from(support & WIKTIONARY != 0),
            f64::from(support & CC_CEDICT != 0),
            word.simplified.chars().count().min(12) as f64,
            word.hsk.map_or(0.0, |level| 1.0 / f64::from(level.max(1))),
            if verb_query {
                if rank.tail_infinitive {
                    1.0
                } else if rank.tail_pos == 1 {
                    0.5
                } else {
                    -1.0
                }
            } else {
                0.0
            },
            f64::from(rank.tail_pos == 1),
            f64::from(rank.tail_pos == 2),
            f64::from(rank.tail_pos == 3),
            f64::from(rank.restricted || rank.proper || rank.regional),
        ]
    }

    pub(super) fn rank_tail(
        &self,
        text: &str,
        matches: &mut Vec<(Relevance, u32)>,
        query: &super::usual::LexicalQuery<'_>,
    ) {
        let (own, marked) = query.marks.unwrap_or((false, &[]));
        // capitals keep only marks whose meanings write them so ("TV" is 电视, "ER" isn't 额)
        let marked: Vec<_> = if self.is_abbreviation(text) {
            marked
                .iter()
                .copied()
                .filter(|&(_, id)| {
                    matches
                        .iter()
                        .any(|&(rank, found)| found == id && rank.kind < MatchKind::Mention)
                })
                .collect()
        } else {
            marked.to_vec()
        };
        let support = &query.support;
        if !support.is_empty() {
            let mut seen: Vec<_> = matches.iter().map(|&(_, id)| id).collect();
            seen.sort_unstable();
            // tables retrieve synonyms in id order, so ties stay stable
            let mut extra = support
                .iter()
                .map(|&(id, _)| id)
                .filter(|id| seen.binary_search(id).is_err())
                .peekable();
            if extra.peek().is_some() {
                let query = self.ranking_query(text, self.is_abbreviation(text));
                matches.extend(extra.map(|id| (self.english_rank(id, &query, false), id)));
            }
        }
        // a marked word needn't repeat the query ("likes" → 喜欢 "to enjoy")
        for &(_, id) in &marked {
            if !matches.iter().any(|&(_, found)| found == id) {
                let mut rank = Relevance::new(MatchKind::Usual);
                rank.form_cost = u8::from(!own);
                matches.push((rank, id));
            }
        }
        let mut scored: Vec<_> = matches
            .drain(..)
            .map(|(mut rank, id)| {
                let mark = marked
                    .iter()
                    .find(|&&(_, marked)| marked == id)
                    .map_or(u8::MAX, |&(order, _)| order);
                if own && mark != u8::MAX {
                    let lead = rank.lead;
                    rank = Relevance::new(MatchKind::Usual);
                    rank.lead = lead;
                    rank.sense_position = mark;
                }
                let bits = support_bits(support, id);
                rank.named |= mark != u8::MAX || bits & (CORE | WIKTIONARY) != 0;
                let class = if mark == u8::MAX {
                    match_class(rank)
                } else {
                    0
                };
                // other forms compete on the learned score, which weighs form; typed english isn't pinyin ("ran" before 然)
                let pinyin = rank.kind == MatchKind::Pinyin;
                let key = (!(own && mark != u8::MAX), class, pinyin, mark);
                (
                    key,
                    score(&self.tail_features(text, rank, id, bits)),
                    rank,
                    id,
                )
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
        matches.extend(scored.into_iter().map(|(_, _, rank, id)| (rank, id)));
    }
}
