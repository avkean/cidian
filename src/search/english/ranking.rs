use super::super::lexicon::{self, lexicon};
use super::super::tail::match_class;
use super::super::{Lead, MatchKind, Relevance, SearchIndex, Word};
use super::{
    Function, abbreviations, capitals, fold_english, function, glosses, has_role, inside_word,
    joined, joined_gloss, name_entry, normalize_folded, prepare_gloss, tokens, without_notes,
    word_list,
};

const LATE_SENSE: u8 = 3;
const MAX_QUALIFIERS: usize = 9;

// costs within a tier in tenths: 10 is a word about 2.7x rarer (see `blend`)
const SENSE_COST: u16 = 80;
const LATER_GLOSS_COST: u16 = 30;
const QUALIFIER_COST: u16 = 25;
const FORM_COST: u16 = 10;
const HIDDEN_COST: u16 = 60;
const HEAD_COST: u16 = 100;
const WEAK_SENSE_COST: u16 = 15;
const WEAK_QUALIFIER_COST: u16 = 3;
const PINYIN_COST: u16 = 30;
const FREQUENCY_SCALE: f64 = 1000.0;
const RELEVANCE_SCALE: i32 = 100;
/// character counts include compound uses, so a similarly common standalone word comes first
const CHARACTER_COST: i32 = 2000;
const BASIC_HSK_BONUS: i32 = 3000;
const INTERMEDIATE_HSK_BONUS: i32 = 1500;
const ADVANCED_HSK_BONUS: i32 = 500;

#[derive(Clone)]
pub(super) struct EnglishPart {
    plain: Box<str>,
    raw: Option<Box<str>>,
    lead: Lead,
    place: u8,
    sense_depth: u16,
    core_sense: bool,
    part_position: u8,
    verb: bool,
    noun: bool,
    adjective: bool,
    note: bool,
    restricted: bool,
    caution: u8,
    hidden: bool,
    search_only: bool,
    literal: bool,
    proper: bool,
    regional: bool,
    possessive: bool,
    capitals: Option<Box<[Box<str>]>>,
}

impl EnglishPart {
    fn new(raw: String, plain: &str) -> Self {
        Self {
            capitals: None,
            verb: raw.trim_start().starts_with("to "),
            raw: (!tokens(&raw).eq(tokens(plain))).then(|| raw.into_boxed_str()),
            plain: plain.into(),
            lead: Lead::None,
            place: 0,
            sense_depth: 0,
            core_sense: false,
            part_position: 0,
            noun: false,
            adjective: false,
            note: false,
            restricted: false,
            caution: 0,
            hidden: false,
            search_only: false,
            literal: false,
            proper: false,
            regional: false,
            possessive: false,
        }
    }

    pub(super) fn prepare(word: &Word) -> Vec<Self> {
        let mut parts = Vec::new();
        // Pointers take no ranking positions; leads keep the original sense index.
        let senses: Vec<_> = word.searchable_senses().collect();
        for (position, &(at, sense)) in senses.iter().enumerate() {
            let lead = Lead::Sense(at.min(u16::MAX as usize) as u16);
            let sense_depth = position.min(u16::MAX as usize) as u16;
            // a noun's first sense shouldn't turn minor just because the entry explains its verb uses first
            let position = if sense.pos.is_some() {
                senses[..position]
                    .iter()
                    .filter(|(_, earlier)| earlier.pos == sense.pos)
                    .count()
            } else {
                position
            };
            let restricted = sense.usage.restricted;
            let pos = sense.pos.as_deref();
            for (part_position, text) in glosses(&sense.text).enumerate() {
                let start = parts.len();
                let (raw, plain, possessive) = prepare_gloss(text);
                let capitals = capitals(text);
                let alternatives: Vec<&str> = plain.split(" or ").collect();
                let has_article = tokens(&raw)
                    .next()
                    .is_some_and(|first| function(first) == Some(Function::Article));
                // short noun alternatives can be whole translations, but don't split shared modifiers
                // ("computer or phone system", "a bitter or wry smile")
                let alternatives = if alternatives.iter().all(|part| tokens(part).count() == 1)
                    || (has_article
                        && tokens(alternatives[0]).count() == 1
                        && !lexicon().adjective(alternatives[0])
                        && alternatives.iter().all(|part| tokens(part).count() <= 2))
                {
                    alternatives
                } else {
                    vec![plain.as_str()]
                };
                for (alternative, plain) in alternatives.into_iter().enumerate() {
                    let part = Self::new(raw.clone(), plain);
                    parts.push(Self {
                        lead,
                        place: if restricted {
                            LATE_SENSE
                        } else {
                            position.min(LATE_SENSE.into()) as u8
                        },
                        sense_depth,
                        core_sense: sense.source == super::Source::Core,
                        restricted,
                        caution: sense.usage.caution,
                        proper: sense.usage.proper,
                        regional: sense.usage.regional,
                        possessive,
                        part_position: (part_position + alternative).min(255) as u8,
                        verb: part.verb || pos == Some("verb"),
                        noun: pos == Some("noun"),
                        adjective: pos == Some("adjective"),
                        note: text.contains(['(', '[']) || sense.usage.formal,
                        literal: text.trim_start().starts_with("lit. "),
                        capitals: capitals.clone(),
                        ..part
                    });
                }
                if let Some(meaning) = parts.last().cloned() {
                    for abbreviation in abbreviations(text) {
                        parts.push(Self {
                            plain: fold_english(abbreviation).into(),
                            raw: None,
                            note: false,
                            ..meaning.clone()
                        });
                    }
                }
                Self::push_joined(&mut parts, start, text, false);
            }
        }
        // search-only glosses have no reliable order or pos, so they all get the same weaker weight, and
        // "X for Y" is only a whole match for X when the gloss is a verb ("classifier for years" isn't one)
        for text in word.search_glosses.iter().flat_map(|text| glosses(text)) {
            let start = parts.len();
            let (raw, plain, possessive) = prepare_gloss(text);
            // a duplicate mustn't promote a late or restricted meaning or drop its qualifiers
            if plain.is_empty() || parts.iter().any(|part| *part.plain == plain) {
                continue;
            }
            parts.push(Self {
                note: text.contains(['(', '[']),
                hidden: true,
                search_only: true,
                capitals: capitals(text),
                // replaced meanings sit after the shown senses instead of borrowing the entry's frequency
                sense_depth: senses.len().min(u16::MAX as usize) as u16,
                core_sense: senses
                    .first()
                    .is_some_and(|(_, sense)| sense.source == super::Source::Core),
                possessive,
                ..Self::new(raw, &plain)
            });
            Self::push_joined(&mut parts, start, text, name_entry(word));
        }
        parts
    }

    fn push_joined(parts: &mut Vec<Self>, start: usize, text: &str, name: bool) {
        let Some(joined_text) = joined_gloss(text).filter(|_| !name) else {
            return;
        };
        let capitals = capitals(&joined_text);
        for index in start..parts.len() {
            let part = &parts[index];
            let Some(plain) = joined(&part.plain).filter(|_| !part.proper) else {
                continue;
            };
            let variant = Self {
                raw: part.raw.as_deref().map(|raw| {
                    let raw = without_notes(raw);
                    joined(&raw).unwrap_or(raw).into_boxed_str()
                }),
                plain: plain.into_boxed_str(),
                capitals: capitals.clone(),
                hidden: true,
                ..part.clone()
            };
            parts.push(variant);
        }
    }

    pub(super) fn heads(&self) -> impl Iterator<Item = &str> {
        let first = tokens(&self.plain).next();
        let last = (!self.verb)
            .then(|| noun_head(&self.plain).last().copied())
            .flatten();
        first.into_iter().chain(last)
    }

    pub(super) fn leads(&self) -> impl Iterator<Item = &str> {
        let mut words = tokens(&self.plain);
        let second = words
            .next()
            .filter(|&first| self.adjective && function(first).is_none() && lexicon().adverb(first))
            .and_then(|_| words.next());
        self.heads().chain(second)
    }

    fn rank(&self, query: &Query, mentions: bool) -> Option<Relevance> {
        let wanted = if self.possessive {
            query.without_possessive.as_deref().unwrap_or(&query.words)
        } else {
            &query.words
        };
        let plain = self.plain.as_ref();
        let words = word_list(plain);
        let starts = |at| !inside_word(plain, &words, at);
        let head = plain.split_once(',').map(|(head, _)| word_list(head));
        // "leaves" is likelier the plural noun than "to leave"
        let misread = |form: u8| query.verb_form && self.noun && form > 0;
        let qualified = !self.proper
            && self.part_position == 0
            && words.get(wanted.len()).is_some_and(|next| {
                function(next) == Some(Function::Qualifier)
                    || (!self.noun
                        && (!self.search_only || self.verb)
                        && function(next) == Some(Function::Purpose))
            });
        let modified_adjective = self.adjective
            && words.len() == wanted.len() + 1
            && function(words[0]).is_none()
            && lexicon().adverb(words[0])
            && starts(1);
        let whole = [
            (words.len() == wanted.len())
                .then(|| matches_start(&words, wanted))
                .flatten(),
            // "London, the capital" names London but "breaded, fried cutlet" isn't bread
            head.filter(|head| head.len() == wanted.len())
                .and_then(|head| matches_start(&head, wanted))
                .filter(|&form| form == 0),
            qualified.then(|| matches_start(&words, wanted)).flatten(),
            modified_adjective
                .then(|| matches_start(&words[1..], wanted))
                .flatten(),
        ]
        .into_iter()
        .flatten()
        .min()
        .filter(|&form| !(self.verb && query.plural && form > 0) && !misread(form));
        let (kind, qualifiers, form, named) = if let Some(form) = whole {
            let kind = if self.part_position > 0 {
                MatchKind::LaterGloss
            } else {
                MatchKind::WholeGloss
            };
            let qualifiers = usize::from(qualified || self.note)
                + usize::from(self.part_position.saturating_sub(1).min(2));
            let named = form < lexicon::AMBIGUOUS_FORM_COST || uses(&words, wanted);
            (kind, qualifiers, form, named)
        } else if let Some(form) = matches_start(&words, wanted) {
            (
                MatchKind::GlossHead,
                words.len() - wanted.len(),
                form,
                uses(&words, wanted),
            )
        } else if let Some((modifiers, form)) = (!self.verb)
            .then(|| head_noun(plain, &words, wanted))
            .flatten()
            .filter(|&(modifiers, _)| starts(modifiers))
        {
            // a negation names the opposite ("not specific", "no longer used")
            let negated = matches!(words[0], "not" | "no" | "non");
            (
                MatchKind::GlossHead,
                modifiers,
                form,
                !negated && uses(&words, wanted),
            )
        } else if mentions {
            let text = self.raw.as_deref().unwrap_or(plain);
            let raw = word_list(text);
            let form = raw
                .windows(wanted.len())
                .enumerate()
                .filter(|&(at, _)| !inside_word(text, &raw, at))
                .filter_map(|(_, run)| matches_start(run, wanted))
                .min()?;
            (
                MatchKind::Mention,
                raw.len() - wanted.len(),
                form,
                uses(&raw, wanted),
            )
        } else {
            return None;
        };
        let written_in_capitals = self.capitals.as_ref().is_some_and(|capitals| {
            wanted
                .iter()
                .all(|wanted| capitals.iter().any(|word| wanted.matches(word).is_some()))
        });
        let form = form.saturating_add(u8::from(written_in_capitals && !query.capitals));
        let partial_name = self.proper && kind >= MatchKind::GlossHead;
        let kind = if partial_name || misread(form) || (query.capitals && !written_in_capitals) {
            MatchKind::Mention
        } else {
            kind
        };
        let whole = matches!(kind, MatchKind::WholeGloss | MatchKind::LaterGloss);
        Some(Relevance {
            kind,
            lead: self.lead,
            sense_depth: 0,
            core_sense: false,
            tail_pos: 0,
            tail_infinitive: false,
            tail_explanation: false,
            restricted: (whole && self.place == LATE_SENSE)
                || (kind == MatchKind::GlossHead && self.restricted),
            sense_position: if whole {
                self.place
            } else {
                self.place.saturating_sub(1)
            },
            qualifiers: qualifiers.min(MAX_QUALIFIERS) as u8,
            form_cost: form,
            hidden: self.hidden,
            regional: self.regional && !query.regional,
            proper: self.proper,
            caution: self.caution,
            // a replaced source's gloss counts as typed or as the base of a typed form ("mended"), not an inflection
            // ("essences" isn't essence)
            named: named
                && !self.literal
                && !(query.capitals && !written_in_capitals)
                && (!self.search_only || form == 0 || query.verb_form || query.plural),
        })
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub(in crate::search) struct Query {
    pub(super) words: Vec<Wanted>,
    pub(super) without_possessive: Option<Vec<Wanted>>,
    verb_form: bool,
    plural: bool,
    regional: bool,
    capitals: bool,
}

impl Query {
    pub(super) fn without_forms_if(mut self, exact: bool) -> Self {
        if exact {
            for word in self
                .words
                .iter_mut()
                .chain(self.without_possessive.iter_mut().flatten())
            {
                word.forms.clear();
            }
            self.verb_form = false;
            self.plural = false;
        }
        self
    }

    pub(super) fn exact_if(self, exact: bool) -> Self {
        Self {
            capitals: exact,
            ..self.without_forms_if(exact)
        }
    }
}

pub(super) fn better(rank: Relevance, other: Relevance) -> bool {
    (match_class(rank), rank_order(rank), rank.regional)
        < (match_class(other), rank_order(other), other.regional)
}

impl SearchIndex {
    pub(in crate::search) fn ranking_query(&self, text: &str, exact: bool) -> Query {
        self.query(&normalize_folded(&fold_english(text)))
            .exact_if(exact)
    }

    pub(in crate::search) fn row_lead(
        &self,
        rank: Relevance,
        id: u32,
        query: &Query,
        keep: bool,
    ) -> Lead {
        match rank.lead {
            Lead::None => self.english_rank(id, query, keep).lead,
            lead => lead,
        }
    }

    pub(in crate::search) fn query(&self, text: &str) -> Query {
        let lexicon = lexicon();
        let words: Vec<Wanted> = word_list(text)
            .into_iter()
            .map(|word| Wanted {
                verb_forms: lexicon.verb_forms(word),
                forms: lexicon
                    .forms(word)
                    .into_iter()
                    .filter(|form| self.english.contains_key(form))
                    .filter_map(|form| {
                        let cost = lexicon.form_cost(word, &form)?;
                        Some((form, cost))
                    })
                    .collect(),
                word: word.to_string(),
            })
            .collect();
        Query {
            without_possessive: (words.len() > 1
                && words.iter().any(|word| has_role(&word.word, "possessive")))
            .then(|| {
                words
                    .iter()
                    .filter(|word| !has_role(&word.word, "possessive"))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .filter(|words| !words.is_empty()),
            verb_form: words.iter().any(|term| lexicon.verb_inflection(&term.word)),
            plural: words.iter().any(|term| lexicon.noun_inflection(&term.word)),
            regional: super::model::has_region(text),
            capitals: false,
            words,
        }
    }

    pub(in crate::search) fn english_rank(&self, id: u32, query: &Query, keep: bool) -> Relevance {
        let mut best = Relevance::new(MatchKind::Substring);
        if query.words.is_empty() {
            return best;
        }
        self.with_meanings(id, keep, |parts| {
            let mut tail = None;
            let mut named = false;
            let mut shown: Option<(Relevance, Lead)> = None;
            for part in parts {
                let mentions = match_class(best) >= match_class(Relevance::new(MatchKind::Mention));
                let Some(rank) = part.rank(query, mentions) else {
                    continue;
                };
                named |= rank.named;
                if !part.search_only && shown.is_none_or(|(other, _)| better(rank, other)) {
                    shown = Some((rank, part.lead));
                }
                if better(rank, best) {
                    best = rank;
                    tail = Some(part);
                }
                if best
                    == (Relevance {
                        named: true,
                        ..Relevance::new(MatchKind::WholeGloss)
                    })
                {
                    break;
                }
            }
            best.named = named;
            // a hidden gloss shows nothing, and a part-of-word match ranks no better than none: lead with the
            // shown meaning that names the query, if any
            if tail.is_none_or(|part| part.search_only) {
                best.lead = shown.map_or(Lead::None, |(_, lead)| lead);
            }
            if let Some(part) = tail {
                best.sense_depth = part.sense_depth;
                best.core_sense = part.core_sense;
                best.tail_pos = if part.verb {
                    1
                } else if part.noun {
                    2
                } else if part.adjective {
                    3
                } else {
                    0
                };
                best.tail_infinitive = part
                    .raw
                    .as_deref()
                    .is_some_and(|raw| raw.trim_start().starts_with("to "));
                best.tail_explanation =
                    best.kind == MatchKind::Mention && !contains_words(&part.plain, &query.words);
            }
            best
        })
    }

    pub(in crate::search) fn blend(&self, rank: Relevance, id: u32) -> i32 {
        let word = &self.words[id as usize];
        let (tier, relevance) = rank_order(rank);
        let familiar = if rank.kind != MatchKind::Pinyin && tier == Tier::Primary {
            match word.hsk {
                Some(1..=3) => BASIC_HSK_BONUS,
                Some(4..=6) => INTERMEDIATE_HSK_BONUS,
                Some(_) => ADVANCED_HSK_BONUS,
                None => 0,
            }
        } else {
            0
        };
        let character_cost = if matches!(rank.kind, MatchKind::WholeGloss | MatchKind::LaterGloss)
            && !rank.restricted
            && word.simplified.chars().count() == 1
            && matches!(
                word.senses.first().and_then(|sense| sense.pos.as_deref()),
                Some("noun" | "adjective")
            ) {
            CHARACTER_COST
        } else {
            0
        };
        let frequency_cost = -((word.frequency.max(0.0).ln_1p() * FREQUENCY_SCALE) as i32);
        let relevance_cost = i32::from(relevance) * RELEVANCE_SCALE;
        frequency_cost + relevance_cost + character_cost - familiar
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::search) enum Tier {
    Usual,
    Primary,
    Restricted(MatchKind),
    PinyinPrefix,
    Other(MatchKind),
}

pub(in crate::search) fn rank_order(rank: Relevance) -> (Tier, u16) {
    let spot = u16::from(rank.sense_position);
    let qualifiers = u16::from(rank.qualifiers);
    let form = u16::from(rank.form_cost) * FORM_COST + if rank.hidden { HIDDEN_COST } else { 0 };
    match rank.kind {
        MatchKind::Usual => (Tier::Usual, spot),
        kind if rank.restricted => (Tier::Restricted(kind), form),
        MatchKind::Pinyin => (Tier::Primary, if rank.proper { 0 } else { PINYIN_COST }),
        MatchKind::PinyinPrefix => (Tier::PinyinPrefix, 0),
        kind @ (MatchKind::WholeGloss | MatchKind::LaterGloss) => (
            Tier::Primary,
            spot * SENSE_COST
                + u16::from(kind == MatchKind::LaterGloss) * LATER_GLOSS_COST
                + qualifiers * QUALIFIER_COST
                + form,
        ),
        MatchKind::GlossHead => (
            Tier::Primary,
            HEAD_COST + spot * WEAK_SENSE_COST + qualifiers * WEAK_QUALIFIER_COST + form,
        ),
        kind => (
            Tier::Other(kind),
            form + spot * WEAK_SENSE_COST + qualifiers * WEAK_QUALIFIER_COST,
        ),
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub(super) struct Wanted {
    pub(super) word: String,
    pub(super) forms: Vec<(String, u8)>,
    verb_forms: Vec<String>,
}

impl Wanted {
    fn names(&self, word: &str) -> bool {
        self.matches(word).is_some_and(|cost| {
            cost < lexicon::AMBIGUOUS_FORM_COST || self.verb_forms.iter().any(|form| form == word)
        })
    }

    fn matches(&self, word: &str) -> Option<u8> {
        if word == self.word {
            Some(0)
        } else {
            self.forms
                .binary_search_by(|(form, _)| form.as_str().cmp(word))
                .ok()
                .map(|index| self.forms[index].1)
        }
    }
}

fn matches_start(words: &[&str], wanted: &[Wanted]) -> Option<u8> {
    if words.len() < wanted.len() {
        return None;
    }
    let mut form = 0;
    for (word, other) in words.iter().zip(wanted) {
        form = form.max(other.matches(word)?);
    }
    Some(form)
}

fn contains_words(text: &str, wanted: &[Wanted]) -> bool {
    let mut words = tokens(text);
    loop {
        let mut run = words.clone();
        if wanted.iter().all(|wanted| {
            run.next()
                .is_some_and(|word| wanted.matches(word).is_some())
        }) {
            return true;
        }
        if words.next().is_none() {
            return false;
        }
    }
}

fn uses(words: &[&str], wanted: &[Wanted]) -> bool {
    words
        .iter()
        .any(|word| wanted.iter().any(|wanted| wanted.names(word)) && !has_role(word, "stop"))
}

fn head_noun(plain: &str, words: &[&str], wanted: &[Wanted]) -> Option<(usize, u8)> {
    if !words
        .windows(wanted.len())
        .any(|run| matches_start(run, wanted).is_some())
    {
        return None;
    }
    let head = noun_head(plain);
    let modifiers = head.len().checked_sub(wanted.len())?;
    (modifiers > 0)
        .then(|| matches_start(&head[modifiers..], wanted))
        .flatten()
        .map(|form| (modifiers, form))
}

fn noun_head(plain: &str) -> Vec<&str> {
    let mut words = tokens(plain.split(',').next().unwrap_or_default());
    let first = words.next();
    first
        .into_iter()
        .chain(words.take_while(|word| function(word).is_none_or(|role| role == Function::Article)))
        .collect()
}
