use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

struct Group {
    pos: u8,
    words: Vec<String>,
}

pub(super) struct Lexicon {
    groups: Vec<Group>,
    by_form: HashMap<String, Vec<usize>>,
    contractions: HashMap<&'static str, &'static str>,
    initialisms: HashMap<&'static str, Vec<&'static str>>,
    joined: HashMap<String, &'static str>,
    derived: HashMap<&'static str, Vec<&'static str>>,
    related: HashMap<&'static str, Vec<&'static str>>,
    words: HashSet<&'static str>,
    counts: Vec<(&'static str, u32)>,
    total: u64,
    clitics: HashSet<&'static str>,
}

pub(super) fn lexicon() -> &'static Lexicon {
    static LEXICON: OnceLock<Lexicon> = OnceLock::new();
    LEXICON.get_or_init(|| {
        let mut result = Lexicon {
            groups: Vec::new(),
            by_form: HashMap::new(),
            contractions: HashMap::new(),
            initialisms: HashMap::new(),
            joined: HashMap::new(),
            derived: HashMap::new(),
            related: HashMap::new(),
            counts: Vec::new(),
            total: 0,
            // the contraction rows leave out a final 's (read as a possessive), so "s" is the one piece they can't supply
            clitics: HashSet::from(["s"]),
            words: include_str!("../../data/english-words.txt")
                .lines()
                .filter(|line| !line.starts_with('#'))
                .collect(),
        };
        let mut readings: HashMap<String, HashSet<&'static str>> = HashMap::new();
        for line in include_str!("../../data/english-usage.tsv").lines() {
            let mut fields = line.split('\t');
            match (fields.next(), fields.next(), fields.next()) {
                (Some(word), Some("F"), Some(count)) => {
                    result.counts.push((word, count.parse().unwrap()));
                }
                (Some(key), Some("A"), Some(full)) => {
                    result.initialisms.insert(key, full.split('|').collect());
                }
                (Some(word), Some("C"), Some(full)) => {
                    result.contractions.insert(word, full);
                    let mut read = |spelling: &str, reading| {
                        readings
                            .entry(spelling.replace('\'', ""))
                            .or_default()
                            .insert(reading);
                    };
                    read(word, word);
                    // halves typed apart when the last ends a word of the full form ("isn", "ve" of "could've")
                    let last = full.rsplit(' ').next().unwrap_or(full);
                    if let Some((head, piece)) = word.rsplit_once('\'')
                        && last != piece
                        && last.ends_with(piece)
                    {
                        read(head, word);
                        read(piece, last);
                    }
                    // a piece that's a word of the full form ("we" of "c'we") stays a word
                    result.clitics.extend(
                        word.split('\'')
                            .skip(1)
                            .filter(|&piece| !piece.is_empty() && piece != last),
                    );
                }
                _ => {}
            }
        }
        // "dont" is "don't" but "well" isn't "we'll", and a spelling with two readings gets neither
        for (spelling, reading) in readings {
            if let [reading] = reading.into_iter().collect::<Vec<_>>()[..]
                && !result.words.contains(spelling.as_str())
            {
                result.joined.insert(spelling, reading);
            }
        }
        result.counts.sort_unstable();
        result.total = result
            .counts
            .iter()
            .map(|&(_, count)| u64::from(count))
            .sum();
        for line in include_str!("../../data/english-lexicon.tsv").lines() {
            if line.starts_with('#') {
                continue;
            }
            let mut fields = line.split('\t');
            let lemma = fields.next().unwrap();
            let pos = fields.next().unwrap().as_bytes()[0];
            // related words are a fallback only, never forms
            if pos == b'D' {
                result.related.insert(lemma, fields.collect());
                continue;
            }
            let mut words = vec![lemma.to_string()];
            for field in fields {
                // an adverb's fields are what it's formed from, not its forms
                if pos == b'R' {
                    result.derived.entry(lemma).or_default().push(field);
                    continue;
                }
                let form = if pos == b'S' {
                    field.to_string()
                } else {
                    let split = field
                        .find(|c: char| !c.is_ascii_digit())
                        .unwrap_or(field.len());
                    let prefix: usize = field[..split].parse().unwrap();
                    format!("{}{}", &lemma[..prefix], &field[split..])
                };
                words.push(form);
            }
            let id = result.groups.len();
            for word in &words {
                result.by_form.entry(word.clone()).or_default().push(id);
            }
            result.groups.push(Group { pos, words });
        }
        // pieces the lexicon knows are words already ("head" of "g'head")
        let Lexicon {
            clitics, by_form, ..
        } = &mut result;
        clitics.retain(|piece| !by_form.contains_key(*piece));
        result
    })
}

pub(super) const AMBIGUOUS_FORM_COST: u8 = 16;

impl Lexicon {
    pub(super) fn lemmas(&self, word: &str, preserve_noun: bool) -> Vec<String> {
        let mut bases = Vec::new();
        for group in self.groups(word) {
            if group.pos == b'S' {
                bases.extend(group.words.iter().cloned());
            } else if !((group.pos == b'A' || (preserve_noun && group.pos == b'V'))
                && self.noun_lemma(word))
            {
                bases.push(group.words[0].clone());
            }
        }
        let variants = bases.clone();
        for base in variants {
            for group in self.groups(&base).filter(|group| group.pos == b'S') {
                bases.extend(group.words.iter().cloned());
            }
        }
        bases.sort_unstable();
        bases.dedup();
        bases.retain(|base| base != word);
        bases
    }

    fn groups(&self, word: &str) -> impl Iterator<Item = &Group> {
        self.by_form
            .get(word)
            .into_iter()
            .flatten()
            .map(|id| &self.groups[*id])
    }

    pub(super) fn forms(&self, word: &str) -> Vec<String> {
        let spellings: HashSet<&str> = std::iter::once(word)
            .chain(
                self.groups(word)
                    .filter(|group| group.pos == b'S')
                    .flat_map(|group| group.words.iter().map(String::as_str)),
            )
            .collect();
        let mut forms = HashSet::new();
        for spelling in spellings {
            forms.insert(spelling.to_string());
            for group in self.groups(spelling) {
                for form in &group.words {
                    forms.insert(form.clone());
                    for variant in self.groups(form).filter(|other| other.pos == b'S') {
                        forms.extend(variant.words.iter().cloned());
                    }
                }
            }
        }
        forms.remove(word);
        let mut forms: Vec<_> = forms.into_iter().collect();
        forms.sort_unstable();
        forms
    }

    pub(super) fn contraction(&self, word: &str) -> Option<&str> {
        self.contractions.get(word).copied()
    }

    pub(super) fn initialisms(&self) -> impl Iterator<Item = (&str, &[&'static str])> {
        self.initialisms
            .iter()
            .map(|(&key, fulls)| (key, fulls.as_slice()))
    }

    pub(super) fn joined_contraction(&self, word: &str) -> Option<&'static str> {
        self.joined.get(word).copied()
    }

    pub(super) fn is_word(&self, word: &str) -> bool {
        self.words.contains(word)
    }

    pub(super) fn count(&self, word: &str) -> u32 {
        self.counts
            .binary_search_by_key(&word, |&(word, _)| word)
            .map_or(0, |index| self.counts[index].1)
    }

    pub(super) fn share(&self, word: &str) -> f64 {
        if self.clitics.contains(word) {
            return 0.0;
        }
        f64::from(self.count(word)) / self.total.max(1) as f64
    }

    pub(super) fn word_groups(&self) -> impl Iterator<Item = &[String]> {
        self.groups.iter().map(|group| group.words.as_slice())
    }

    pub(super) fn knows(&self, word: &str) -> bool {
        self.by_form.contains_key(word)
    }

    pub(super) fn same_spelling(&self, word: &str, other: &str) -> bool {
        word == other
            || self
                .groups(word)
                .any(|group| group.pos == b'S' && group.words.iter().any(|w| w == other))
    }

    pub(super) fn adverb(&self, word: &str) -> bool {
        self.groups(word).any(|group| group.pos == b'R')
    }

    pub(super) fn adverb_bases(&self, word: &str) -> &[&'static str] {
        self.derived.get(word).map_or(&[], Vec::as_slice)
    }

    pub(super) fn related(&self, word: &str) -> Vec<&'static str> {
        std::iter::once(word.to_string())
            .chain(self.lemmas(word, false))
            .filter_map(|word| self.related.get(word.as_str()))
            .flatten()
            .copied()
            .collect()
    }

    pub(super) fn adjective(&self, word: &str) -> bool {
        self.groups(word)
            .any(|group| group.pos == b'A' && group.words[0] == word)
    }

    pub(super) fn noun_lemma(&self, word: &str) -> bool {
        self.groups(word)
            .any(|group| group.pos == b'N' && group.words[0] == word)
    }

    pub(super) fn form_cost(&self, word: &str, form: &str) -> Option<u8> {
        if self.same_spelling(word, form) {
            return Some(0);
        }
        let base = |group: &Group| group.pos != b'S' && group.words[0] == form;
        let own_word = self
            .groups(form)
            .any(|group| base(group) && !group.words.iter().any(|w| self.same_spelling(word, w)));
        if own_word && !self.groups(word).any(base) {
            let regular = form.strip_prefix(word).is_some_and(|ending| {
                matches!(
                    ending,
                    "s" | "es" | "d" | "ed" | "ing" | "r" | "er" | "st" | "est"
                )
            });
            return (!regular || self.count(form) <= self.count(word))
                .then_some(AMBIGUOUS_FORM_COST);
        }
        let noun = self.noun_lemma(word) || self.noun_lemma(form);
        self.groups(word)
            .filter(|group| group.words.iter().any(|other| other == form))
            .map(|group| {
                if noun && group.pos == b'A' {
                    AMBIGUOUS_FORM_COST
                } else {
                    1
                }
            })
            .min()
            .or(Some(1))
    }

    pub(super) fn verb_forms(&self, word: &str) -> Vec<String> {
        self.groups(word)
            .filter(|group| group.pos == b'V' && group.words[0] == word)
            .flat_map(|group| group.words[1..].iter().cloned())
            .collect()
    }

    pub(super) fn verb_inflection(&self, word: &str) -> bool {
        !self.noun_inflection(word)
            && self
                .groups(word)
                .any(|group| group.pos == b'V' && group.words[0] != word)
    }

    pub(super) fn inflection(&self, word: &str) -> bool {
        self.groups(word)
            .any(|group| group.pos != b'S' && group.words[0] != word)
    }

    pub(super) fn noun_inflection(&self, word: &str) -> bool {
        self.groups(word)
            .any(|group| group.pos == b'N' && group.words[0] != word)
    }
}
