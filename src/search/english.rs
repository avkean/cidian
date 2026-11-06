pub(in crate::search) mod fulltext;
mod ranking;
mod retrieval;

use super::lexicon::lexicon;
use super::pinyin::{has_wildcard, tone_mark};
use super::{Lead, MatchKind, Relevance, SearchIndex, Source, Word, is_hanzi, model, tail, usual};
pub(super) use fulltext::{FullText, fulltext_query};
pub(super) use ranking::{Tier, rank_order};
pub(super) use retrieval::{EnglishQuery, MeaningsCache, RANKING_WORK};
use std::collections::HashMap;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

static WORD_ROLES: LazyLock<HashMap<&str, &str>> =
    LazyLock::new(|| word_roles(include_str!("../../data/english-stopwords.tsv")));

pub(super) fn word_roles(text: &str) -> HashMap<&str, &str> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|(number, line)| {
            let mut fields = line.split('\t').map(str::trim);
            match (fields.next(), fields.next(), fields.next()) {
                (Some(word), Some(roles), None) => (word, roles),
                _ => panic!(
                    "english-stopwords.tsv:{}: expected 2 tab-separated fields",
                    number + 1
                ),
            }
        })
        .collect()
}

pub(super) fn has_role(word: &str, role: &str) -> bool {
    WORD_ROLES
        .get(word)
        .is_some_and(|roles| roles.split(',').any(|found| found.trim() == role))
}

#[derive(Clone, Copy, PartialEq)]
enum Function {
    Qualifier,
    Purpose,
    Article,
    Boundary,
}

fn function(word: &str) -> Option<Function> {
    WORD_ROLES
        .get(word)?
        .split(',')
        .map(str::trim)
        .find_map(|role| match role {
            "function:qualifier" => Some(Function::Qualifier),
            "function:purpose" => Some(Function::Purpose),
            "function:article" => Some(Function::Article),
            "function:boundary" => Some(Function::Boundary),
            _ => None,
        })
}

fn leading_words(text: &str) -> &str {
    for prefix in ["to be ", "to ", "be "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return rest;
        }
    }
    if let Some((first, rest)) = text.split_once(' ')
        && function(first) == Some(Function::Article)
    {
        return rest;
    }
    text
}

/// runs before apostrophe folding so "one child" keeps its numeral
fn without_generic_possessive(text: &str) -> (String, bool) {
    let text = without_notes(text).replace('’', "'");
    let mut possessive = false;
    let mut previous: Option<&str> = None;
    let words: Vec<_> = text
        .split_whitespace()
        .filter(|word| {
            let generic = (word.eq_ignore_ascii_case("one's")
                || word.eq_ignore_ascii_case("someone's"))
                && previous.is_some_and(|word| function(&word.to_ascii_lowercase()).is_none());
            previous = Some(word);
            possessive |= generic;
            !generic
        })
        .collect();
    (words.join(" "), possessive)
}

pub(super) fn abbreviation(text: &str) -> bool {
    text.chars().filter(char::is_ascii_alphabetic).count() > 1
        && !text.chars().any(char::is_lowercase)
}

impl SearchIndex {
    pub(super) fn is_abbreviation(&self, text: &str) -> bool {
        abbreviation(text)
            && tokens(&fold_english(text)).all(|token| self.abbreviations.contains_key(token))
    }

    pub(super) fn link_initialisms(&mut self) {
        let mut found = HashMap::new();
        for (key, fulls) in lexicon().initialisms() {
            let mut rows: Vec<(u32, Lead)> = Vec::new();
            for full in fulls {
                let mut matches = self.english_matches(full, false);
                self.rank(&mut matches, true);
                self.rank_tail(full, &mut matches, &self.lexical_query(full));
                let wanted = self.ranking_query(full, false);
                for (rank, id) in matches {
                    if matches!(
                        rank.kind,
                        MatchKind::Usual | MatchKind::WholeGloss | MatchKind::LaterGloss
                    ) && !rows.iter().any(|&(found, _)| found == id)
                    {
                        let lead = self.row_lead(rank, id, &wanted, false);
                        rows.push((id, lead));
                    }
                }
            }
            if !rows.is_empty() {
                found.insert(key, rows);
            }
        }
        self.initialisms = found;
        // matching them is not a reason to keep meanings in memory
        *self.meanings.lock().unwrap() = MeaningsCache::default();
    }

    pub(super) fn spelled_out(&self, text: &str, named: bool) -> &[(u32, Lead)] {
        let key = text.to_ascii_lowercase();
        if named && lexicon().is_word(&key) || !key.bytes().all(|b| b.is_ascii_lowercase()) {
            return &[];
        }
        self.initialisms
            .get(key.as_str())
            .map_or(&[], Vec::as_slice)
    }

    pub(super) fn add_spelled_out(
        &self,
        matches: &mut Vec<(Relevance, u32)>,
        spelled_out: &[(u32, Lead)],
    ) {
        if spelled_out.is_empty() {
            return;
        }
        let asked = |rank: &Relevance| {
            rank.relevant() || (MatchKind::Usual..=MatchKind::Mention).contains(&rank.kind)
        };
        let at = matches
            .iter()
            .rposition(|(rank, _)| asked(rank))
            .map_or(0, |at| at + 1);
        let mut rows: Vec<(Relevance, u32)> = spelled_out
            .iter()
            .filter(|&&(id, _)| {
                !matches[..at]
                    .iter()
                    .any(|&(rank, found)| found == id && asked(&rank))
            })
            .map(|&(id, lead)| {
                let mut rank = Relevance::new(MatchKind::WholeGloss);
                rank.named = true;
                rank.lead = lead;
                (rank, id)
            })
            .collect();
        let mut rest = matches.split_off(at);
        rest.retain(|(_, id)| !rows.iter().any(|&(_, row)| row == *id));
        matches.append(&mut rows);
        matches.append(&mut rest);
    }
}

pub(super) fn may_be_english(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_alphabetic())
        && !has_wildcard(text)
        && !text.nfc().any(|c| matches!(c, 'ü' | 'Ü'))
        && !text.to_ascii_lowercase().contains("u:")
        && !text.nfd().any(|c| is_hanzi(c) || tone_mark(c).is_some())
}

pub(super) fn fold_english(text: &str) -> String {
    let folded: String = if text.is_ascii() {
        text.to_ascii_lowercase()
    } else {
        text.to_lowercase()
            .nfd()
            .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
            .map(|c| if c == '’' { '\'' } else { c })
            .collect()
    };
    if !folded.contains('\'') {
        return folded;
    }
    let folded = expand_contractions(&folded).unwrap_or(folded);
    let chars: Vec<char> = folded.chars().collect();
    let letter = |index: usize| chars.get(index).is_some_and(|c| c.is_alphanumeric());
    let mut output = String::with_capacity(folded.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\'' && index > 0 && letter(index - 1) {
            if chars.get(index + 1) == Some(&'s') && !letter(index + 2) {
                index += 2;
                continue;
            }
            if !letter(index + 1) {
                index += 1;
                continue;
            }
        }
        output.push(chars[index]);
        index += 1;
    }
    output
}

fn expand_contractions(text: &str) -> Option<String> {
    let lexicon = lexicon();
    let mut output = String::new();
    let mut last = 0;
    let mut start = None;
    for (index, c) in text.char_indices().chain([(text.len(), ' ')]) {
        if c.is_alphanumeric() || c == '\'' {
            start.get_or_insert(index);
            continue;
        }
        let Some(begin) = start.take() else {
            continue;
        };
        let run = &text[begin..index];
        let word = run.trim_matches('\'');
        let begin = begin + run.len() - run.trim_start_matches('\'').len();
        if word.contains('\'')
            && let Some(full) = lexicon.contraction(word)
        {
            output.push_str(&text[last..begin]);
            output.push_str(full);
            last = begin + word.len();
        }
    }
    (last > 0).then(|| output + &text[last..])
}

pub(super) fn with_apostrophes(text: &str) -> Option<String> {
    let mut output = String::with_capacity(text.len() + 2);
    let mut last = 0;
    for word in text.split(|c: char| !c.is_alphanumeric() && !matches!(c, '\'' | '’')) {
        if abbreviation(word) {
            continue;
        }
        if let Some(contraction) = lexicon().joined_contraction(&word.to_ascii_lowercase()) {
            let start = word.as_ptr() as usize - text.as_ptr() as usize;
            output.push_str(&text[last..start]);
            output.push_str(contraction);
            last = start + word.len();
        }
    }
    (last > 0).then(|| output + &text[last..])
}

fn after_field_labels(text: &str) -> &str {
    let mut depth = 0usize;
    for (at, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 && c.is_alphanumeric() => return &text[at..],
            _ => {}
        }
    }
    ""
}

fn abbreviations(text: &str) -> impl Iterator<Item = &str> {
    let text = after_field_labels(text);
    text.match_indices('(').filter_map(move |(at, _)| {
        // "iron(II)" is a word with a note
        if text[..at].ends_with(char::is_alphanumeric) {
            return None;
        }
        let rest = &text[at + 1..];
        let end = rest.find(['(', ')'])?;
        let inner = rest[..end].trim();
        (rest[end..].starts_with(')')
            && inner.len() <= 10
            && inner.chars().filter(char::is_ascii_uppercase).count() >= 2
            && inner
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '&'))
        .then_some(inner)
    })
}

fn capital_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| abbreviation(word))
}

pub(super) fn written_abbreviations(text: &str) -> impl Iterator<Item = &str> {
    glosses(text).flat_map(|text| capital_tokens(after_field_labels(text)))
}

fn capitals(text: &str) -> Option<Box<[Box<str>]>> {
    if text.bytes().filter(u8::is_ascii_uppercase).count() < 2 {
        return None;
    }
    let words: Box<[Box<str>]> = capital_tokens(text)
        .map(|word| word.to_ascii_lowercase().into())
        .collect();
    (!words.is_empty()).then_some(words)
}

fn without_notes(text: &str) -> String {
    if !text.contains(['(', ')', '[', ']']) {
        return text.to_string();
    }
    let mut plain = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    plain
}

fn prepare_gloss(text: &str) -> (String, String, bool) {
    let (plain, possessive) = without_generic_possessive(text);
    (
        fold_english(text),
        normalize_plain(&fold_english(&plain)),
        possessive,
    )
}

pub(super) fn normalize_folded(text: &str) -> String {
    normalize_plain(&without_notes(text))
}

fn normalize_plain(text: &str) -> String {
    let plain = text
        .trim_matches(|c: char| !c.is_alphanumeric())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    leading_words(&plain).to_string()
}

pub(super) fn tokens(text: &str) -> impl DoubleEndedIterator<Item = &str> + Clone {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty() && !word.chars().any(is_hanzi))
}

pub(super) fn word_list(text: &str) -> Vec<&str> {
    tokens(text).collect()
}

const JOINERS: &[char] = &['-', '\u{2010}', '\u{2011}', '\u{2013}', '.'];

fn joins(before: char, joiner: char, after: char) -> bool {
    JOINERS.contains(&joiner)
        && before.is_alphanumeric()
        && after.is_alphanumeric()
        && !(before.is_numeric() && after.is_numeric())
}

pub(super) fn joined(text: &str) -> Option<String> {
    if !text.contains(JOINERS) {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let inside = |index: usize| {
        index > 0
            && chars
                .get(index + 1)
                .is_some_and(|&next| joins(chars[index - 1], chars[index], next))
    };
    let joined: String = (0..chars.len())
        .filter(|&index| !inside(index))
        .map(|index| chars[index])
        .collect();
    (joined.len() < text.len()).then_some(joined)
}

fn inside_word(text: &str, words: &[&str], at: usize) -> bool {
    let (Some(before), Some(after)) = (at.checked_sub(1).and_then(|i| words.get(i)), words.get(at))
    else {
        return false;
    };
    let end = before.as_ptr() as usize - text.as_ptr() as usize + before.len();
    let start = after.as_ptr() as usize - text.as_ptr() as usize;
    let mut between = text[end..start].chars();
    match (between.next(), between.next()) {
        (Some(joiner), None) if JOINERS.contains(&joiner) => before
            .chars()
            .next_back()
            .zip(after.chars().next())
            .is_some_and(|(before, after)| joins(before, joiner, after)),
        (Some(bracket), _) => matches!(bracket, '(' | '['),
        _ => false,
    }
}

fn glosses(text: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0usize;
    text.split(move |c: char| {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        c == ';' && depth == 0
    })
}

pub(super) fn joined_gloss(text: &str) -> Option<String> {
    joined(&without_notes(text))
}

pub(super) fn name_entry(word: &Word) -> bool {
    let mut meanings = word.searchable_senses().map(|(_, sense)| sense);
    word.searchable_senses().next().is_some() && meanings.all(|sense| sense.usage.proper)
}

pub(super) fn initials(text: &str) -> impl Iterator<Item = String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '.')
        .map(|word| word.trim_matches('.'))
        .filter(|word| {
            word.contains('.')
                && word
                    .split('.')
                    .all(|letter| letter.len() == 1 && letter.as_bytes()[0].is_ascii_lowercase())
        })
        .map(|word| word.replace('.', ""))
}
