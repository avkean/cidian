use super::english::{fold_english, initials, tokens};
use super::{
    EnglishPostings, Example, MeaningsCache, NEWS_CORROBORATION_FLOOR, SearchIndex, Source,
    SourceCount, Word, english, is_hanzi, lexicon, pinyin, spelling, tail,
};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use unicode_normalization::UnicodeNormalization;

const CHARACTER_NEWS_GAP: f64 = 5.0;
/// shorter readings stay one word, as dictionaries write them (túshūguǎn, yīlùshùnfēng)
const WORD_SPLIT_MIN: usize = 5;

impl SearchIndex {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let mut words = Vec::new();
        let mut spans = Vec::new();
        let mut reader = BufReader::new(File::open(path)?);
        let (mut row, mut offset) = (String::new(), 0);
        for line in 0.. {
            row.clear();
            let read = reader.read_line(&mut row)?;
            if read == 0 {
                break;
            }
            let start = offset;
            offset += read as u64;
            if row.trim().is_empty() {
                continue;
            }
            let mut word: Word = serde_json::from_str(&row).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} line {}: {error}", path.display(), line + 1),
                )
            })?;
            if word.simplified.is_empty()
                || word.senses.is_empty()
                || (!word.frequency.is_finite()
                    || !word.reading_frequency.is_finite()
                    || !word.reading_corpus_frequency.is_finite())
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{} line {}: a word needs a headword, meanings and a finite score",
                        path.display(),
                        line + 1
                    ),
                ));
            }
            // dropped line by line, so the next line reuses their memory
            spans.push((
                start,
                if word.examples.is_empty() {
                    0
                } else {
                    read as u32
                },
            ));
            word.examples = Vec::new();
            words.push(word);
        }
        if words.is_empty() || u32::try_from(words.len()).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Dictionary must contain between 1 and 4,294,967,295 words",
            ));
        }
        let mut index = Self::build(words);
        let support_path = path.with_file_name("english-support.tsv");
        index.support = tail::SupportIndex::read(
            BufReader::new(File::open(&support_path).map_err(|e| {
                io::Error::new(e.kind(), format!("{}: {e}", support_path.display()))
            })?),
            &index.words,
        )?;
        index.link_initialisms();
        index.lines = Some((reader.into_inner(), spans));
        Ok(index)
    }

    pub(super) fn examples<'a>(&self, word: &'a Word) -> Cow<'a, [Example]> {
        #[derive(serde::Deserialize)]
        struct Examples {
            examples: Vec<Example>,
        }
        let Some((start, len)) = self
            .lines
            .as_ref()
            .map(|(_, spans)| spans[word.id as usize])
        else {
            return Cow::Borrowed(&word.examples);
        };
        if len == 0 {
            return Cow::Borrowed(&[]);
        }
        let mut line = vec![0; len as usize];
        let file = &self.lines.as_ref().unwrap().0;
        let read = std::os::unix::fs::FileExt::read_exact_at(file, &mut line, start)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                serde_json::from_slice::<Examples>(&line).map_err(|error| error.to_string())
            });
        match read {
            Ok(parsed) => Cow::Owned(parsed.examples),
            Err(error) => {
                if !self
                    .examples_failed
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    eprintln!("Could not read examples from the dictionary file: {error}");
                }
                Cow::Borrowed(&[])
            }
        }
    }

    pub fn with_examples<'a>(&self, word: &'a Word) -> Cow<'a, Word> {
        match self.examples(word) {
            Cow::Borrowed(_) => Cow::Borrowed(word),
            Cow::Owned(examples) => Cow::Owned(Word {
                examples,
                ..word.clone()
            }),
        }
    }

    fn build(mut words: Vec<Word>) -> Self {
        lexicon::lexicon();
        let mut index = Self {
            words: Vec::new(),
            usual: HashMap::new(),
            support: tail::SupportIndex::default(),
            headwords: BTreeMap::new(),
            loose: HashMap::new(),
            longest_headword: 0,
            log_total: 0.0,
            pinyin: BTreeMap::new(),
            syllables: HashSet::new(),
            reading_total: 0.0,
            unsupported_characters: HashSet::new(),
            english: BTreeMap::new(),
            characters: HashMap::new(),
            abbreviations: HashMap::new(),
            initials: HashSet::new(),
            initialisms: HashMap::new(),
            speller: spelling::Speller::default(),
            meanings: Mutex::new(MeaningsCache::default()),
            ranked: Mutex::default(),
            fulltext: OnceLock::new(),
            lines: None,
            examples_failed: std::sync::atomic::AtomicBool::new(false),
        };
        let mut english: HashMap<String, Vec<u32>> = HashMap::new();
        let mut joined_only: HashMap<String, Vec<u32>> = HashMap::new();
        for (i, word) in words.iter_mut().enumerate() {
            let id = i as u32;
            word.id = id;
            for (query, order) in std::mem::take(&mut word.usual_for) {
                index.usual.entry(query).or_default().push((order, id));
            }
            // serde often reserves room for four, and these never change
            word.senses.shrink_to_fit();
            word.traditional.shrink_to_fit();
            word.classifiers.shrink_to_fit();
            word.sources.sort_unstable();
            word.sources.dedup();
            word.sources.shrink_to_fit();
            for sense in &mut word.senses {
                sense.normalize_usage();
                sense.tags.shrink_to_fit();
            }
            let mut forms = vec![word.simplified.as_str()];
            forms.extend(
                word.traditional
                    .iter()
                    .map(String::as_str)
                    .filter(|form| !form.is_empty()),
            );
            forms.sort_unstable();
            forms.dedup();
            let mut characters = HashSet::new();
            for form in &forms {
                index
                    .headwords
                    .entry(form.to_string())
                    .or_default()
                    .push(id);
                let key = loose_key(form);
                if !key.is_empty() && key != *form {
                    index.loose.entry(key).or_default().push(id);
                }
                index.longest_headword = index.longest_headword.max(form.chars().count());
                characters.extend(form.chars().filter(|c| is_hanzi(*c)));
            }
            for c in characters {
                index.characters.entry(c).or_default().push(id);
            }
            // a neutral syllable is pinyin when it reads a character ("me" of 什么), not a letter ("D" of 3D打印)
            let syllables: Vec<&str> = word.pinyin.split_whitespace().collect();
            let characters: Vec<char> = word.simplified.chars().collect();
            index.syllables.extend(
                syllables
                    .iter()
                    .enumerate()
                    .filter(|&(at, syllable)| {
                        syllable.ends_with(|c: char| c.is_ascii_digit())
                            || (syllables.len() == characters.len() && is_hanzi(characters[at]))
                    })
                    .map(|(_, syllable)| pinyin::letters(syllable)),
            );
            let pinyin = pinyin::letters(&word.pinyin);
            if !pinyin.is_empty() {
                index.pinyin.entry(pinyin).or_default().push(id);
            }
            let folded: Vec<String> = word
                .searchable_senses()
                .map(|(_, sense)| sense)
                .map(|sense| sense.text.as_str())
                .chain(word.search_glosses.iter().map(String::as_str))
                .map(fold_english)
                .collect();
            let name = english::name_entry(word);
            let joined_folded: Vec<String> = word
                .searchable_senses()
                .map(|(_, sense)| sense)
                .filter(|sense| !sense.usage.proper)
                .map(|sense| sense.text.as_str())
                .chain(
                    word.search_glosses
                        .iter()
                        .filter(|_| !name)
                        .map(String::as_str),
                )
                .filter_map(english::joined_gloss)
                .map(|text| fold_english(&text))
                .collect();
            let abbreviations: HashSet<_> = word
                .searchable_senses()
                .map(|(_, sense)| sense)
                .flat_map(|sense| english::written_abbreviations(&sense.text))
                .collect();
            for token in abbreviations {
                index
                    .abbreviations
                    .entry(token.to_ascii_lowercase())
                    .or_default()
                    .push(id);
            }
            for sense in word.searchable_senses().map(|(_, sense)| sense) {
                index.initials.extend(initials(&sense.text));
            }
            let terms: HashSet<&str> = folded.iter().flat_map(|text| tokens(text)).collect();
            let joined_terms: HashSet<&str> = joined_folded
                .iter()
                .flat_map(|text| tokens(text))
                .filter(|token| !terms.contains(token))
                .collect();
            for token in terms.into_iter().chain(joined_terms.iter().copied()) {
                english.entry(token.to_string()).or_default().push(id);
            }
            for token in joined_terms {
                joined_only.entry(token.to_string()).or_default().push(id);
            }
        }
        let lexicon = lexicon::lexicon();
        let used = |term: &String| {
            joined_only
                .get(term)
                .is_none_or(|ids| ids.len() < english[term].len())
        };
        let vocabulary = english
            .keys()
            .filter(|term| used(term))
            .map(String::as_str)
            .chain(
                lexicon
                    .word_groups()
                    .filter(|group| {
                        group
                            .iter()
                            .any(|word| english.contains_key(word) && used(word))
                    })
                    .flatten()
                    .map(String::as_str),
            );
        index.speller = spelling::Speller::new(vocabulary);
        index.english = english
            .into_iter()
            .map(|(term, mut all)| {
                all.shrink_to_fit();
                let joined = joined_only.remove(&term).unwrap_or_default();
                (
                    term,
                    EnglishPostings {
                        all,
                        joined,
                        heads: OnceLock::new(),
                        leads: OnceLock::new(),
                    },
                )
            })
            .collect();
        for ids in index
            .headwords
            .values_mut()
            .chain(index.pinyin.values_mut())
            .chain(index.characters.values_mut())
            .chain(index.abbreviations.values_mut())
        {
            ids.shrink_to_fit();
        }
        index.log_total = words
            .iter()
            .map(|word| word.frequency.max(0.0))
            .sum::<f64>()
            .max(1.0)
            .ln();
        index.reading_total = words
            .iter()
            .map(|word| word.reading_frequency.max(0.0))
            .sum();
        words.shrink_to_fit();
        for ids in index.usual.values_mut() {
            ids.sort_unstable();
            ids.shrink_to_fit();
        }
        for ids in index.pinyin.values() {
            let singles: Vec<_> = ids
                .iter()
                .copied()
                .filter(|&id| words[id as usize].simplified.chars().count() == 1)
                .collect();
            let best = singles
                .iter()
                .map(|&id| words[id as usize].reading_corpus_frequency)
                .fold(0.0_f64, f64::max);
            index
                .unsupported_characters
                .extend(singles.into_iter().filter(|&id| {
                    let count = words[id as usize].reading_corpus_frequency;
                    count < NEWS_CORROBORATION_FLOOR && count * CHARACTER_NEWS_GAP < best
                }));
        }
        index.words = words;
        let starts: Vec<u32> = index
            .words
            .iter()
            .map(|word| index.word_starts(word))
            .collect();
        for (word, starts) in index.words.iter_mut().zip(starts) {
            word.word_starts = starts;
        }
        index
    }

    fn word_starts(&self, word: &Word) -> u32 {
        let characters = word.simplified.chars().count();
        if !word.written.is_empty()
            || !(WORD_SPLIT_MIN..=u32::BITS as usize).contains(&characters)
            || !word.simplified.chars().all(is_hanzi)
            || word.pinyin.split_whitespace().count() != characters
        {
            return 0;
        }
        let parts = self.split(&word.simplified, characters - 1);
        if parts
            .iter()
            .any(|(text, ids)| ids.is_empty() || text.chars().count() < 2)
        {
            return 0;
        }
        let mut starts = 0;
        let mut at = 0;
        for (text, _) in &parts[..parts.len() - 1] {
            at += text.chars().count();
            starts |= 1 << at;
        }
        starts
    }

    pub fn get(&self, id: u32) -> Option<&Word> {
        self.words.get(id as usize)
    }

    pub fn syllables(&self) -> impl Iterator<Item = &str> {
        self.syllables.iter().map(String::as_str)
    }

    pub fn source_counts(&self) -> Vec<SourceCount> {
        let mut counts: BTreeMap<Source, usize> = BTreeMap::new();
        for word in &self.words {
            for source in &word.sources {
                *counts.entry(*source).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .map(|(source, words)| SourceCount {
                id: source.id().to_string(),
                words,
            })
            .collect()
    }

    pub(super) fn by_headword(&self, word: &str) -> &[u32] {
        self.headwords.get(word).map_or(&[], Vec::as_slice)
    }

    pub(super) fn exact(&self, text: &str) -> &[u32] {
        let ids = self.by_headword(text);
        if !ids.is_empty() {
            return ids;
        }
        self.loose.get(&loose_key(text)).map_or(&[], Vec::as_slice)
    }
}

fn loose_key(text: &str) -> String {
    text.nfkc()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
