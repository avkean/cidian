use crate::search::{Word, is_hanzi};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Conversion is for display; search stays simplified.
pub struct Traditional {
    /// empty target means the character fallback
    phrases: Vec<(Box<str>, Box<str>)>,
    characters: HashMap<char, char>,
    max_chars: usize,
}

impl Traditional {
    pub fn new(words: &[Word]) -> Self {
        // Reading use picks the common character form when word spellings disagree.
        let mut weights: BTreeMap<(char, char), (f64, u32)> = BTreeMap::new();
        for word in words {
            let Some(traditional) = word.traditional.first() else {
                continue;
            };
            if traditional.chars().count() != word.simplified.chars().count() {
                continue;
            }
            for pair in word.simplified.chars().zip(traditional.chars()) {
                if is_hanzi(pair.0) {
                    let weight = weights.entry(pair).or_default();
                    weight.0 += word.reading_corpus_frequency;
                    weight.1 += 1;
                }
            }
        }
        let mut best: HashMap<char, (char, (f64, u32))> = HashMap::new();
        let mut ambiguous = HashSet::new();
        for ((from, to), weight) in weights {
            let current = best.entry(from).or_insert((to, weight));
            if current.0 != to {
                ambiguous.insert(from);
            }
            if weight > current.1 {
                *current = (to, weight);
            }
        }
        let mut characters: HashMap<char, char> =
            best.into_iter().map(|(from, (to, _))| (from, to)).collect();
        for line in include_str!("../data/traditional-kept.tsv").lines() {
            if let Some((from, to)) = line.split_once('\t').filter(|_| !line.starts_with('#')) {
                characters.extend(from.chars().zip(to.chars()));
            }
        }
        characters.retain(|from, to| from != to);

        let mut spellings: BTreeMap<&str, (&str, f64, f64)> = BTreeMap::new();
        for word in words {
            let Some(traditional) = word.traditional.first() else {
                continue;
            };
            if word.simplified.chars().nth(1).is_none() {
                continue;
            }
            let candidate = (
                traditional.as_str(),
                word.reading_corpus_frequency,
                word.reading_frequency,
            );
            let current = spellings
                .entry(word.simplified.as_str())
                .or_insert(candidate);
            if (candidate.1, candidate.2) > (current.1, current.2) {
                *current = candidate;
            }
        }
        // Preserve existing traditional spellings, including those in example sentences.
        for word in words {
            for traditional in &word.traditional {
                if traditional.chars().nth(1).is_some() {
                    spellings
                        .entry(traditional)
                        .or_insert((traditional, 0.0, 0.0));
                }
            }
        }
        let fallback = |text: &str| -> String {
            text.chars()
                .map(|c| characters.get(&c).copied().unwrap_or(c))
                .collect::<String>()
        };
        // Ambiguous words block overlapping replacements; unambiguous words must leave room for them.
        let phrases: Vec<(Box<str>, Box<str>)> = spellings
            .into_iter()
            .filter_map(|(from, (to, ..))| {
                if fallback(from) != to {
                    Some((from.into(), to.into()))
                } else if from.chars().any(|c| ambiguous.contains(&c)) {
                    Some((from.into(), "".into()))
                } else {
                    None
                }
            })
            .collect();
        let max_chars = phrases
            .iter()
            .map(|(from, _)| from.chars().count())
            .max()
            .unwrap_or(0);
        Self {
            phrases,
            characters,
            max_chars,
        }
    }

    fn character(&self, c: char) -> char {
        self.characters.get(&c).copied().unwrap_or(c)
    }

    fn convert(&self, text: &str) -> String {
        let mut converted = String::with_capacity(text.len());
        let mut ends = Vec::with_capacity(self.max_chars);
        let mut rest = text;
        while let Some(first) = rest.chars().next() {
            let mut length = first.len_utf8();
            if is_hanzi(first) {
                ends.clear();
                ends.extend(
                    rest.char_indices()
                        .skip(1)
                        .take(self.max_chars.saturating_sub(1))
                        .map(|(i, c)| i + c.len_utf8()),
                );
                let phrase = ends.iter().rev().find_map(|&end| {
                    self.phrases
                        .binary_search_by(|(form, _)| form.as_ref().cmp(&rest[..end]))
                        .ok()
                        .map(|index| (end, &self.phrases[index].1))
                });
                if let Some((end, target)) = phrase {
                    length = end;
                    if !target.is_empty() {
                        converted.push_str(target);
                        rest = &rest[length..];
                        continue;
                    }
                }
                for c in rest[..length].chars() {
                    converted.push(self.character(c));
                }
            } else {
                converted.push(first);
            }
            rest = &rest[length..];
        }
        converted
    }

    /// Keep source text intact; attach alternatives only to the objects that display it.
    pub fn display(&self, content: &impl Serialize) -> Value {
        let mut value = serde_json::to_value(content).expect("Display content is serializable");
        self.annotate(&mut value);
        value
    }

    fn annotate(&self, value: &mut Value) {
        match value {
            Value::Array(items) => {
                for item in items {
                    self.annotate(item);
                }
            }
            Value::Object(object) => {
                let mut alternatives = Map::new();
                for (key, value) in object.iter_mut() {
                    match key.as_str() {
                        "text" | "note" | "chinese" | "definition" | "pos" => {
                            if let Some(text) = value.as_str() {
                                self.alternative(text, &mut alternatives);
                            }
                        }
                        "tags" | "classifiers" | "gloss" => {
                            if let Some(items) = value.as_array() {
                                for text in items.iter().filter_map(Value::as_str) {
                                    self.alternative(text, &mut alternatives);
                                }
                            }
                        }
                        _ => self.annotate(value),
                    }
                }
                if !alternatives.is_empty() {
                    object.insert("traditional_text".into(), Value::Object(alternatives));
                }
            }
            _ => {}
        }
    }

    fn alternative(&self, text: &str, alternatives: &mut Map<String, Value>) {
        if !text.chars().any(is_hanzi) {
            return;
        }
        let converted = self.convert(text);
        if converted != text {
            alternatives.insert(text.into(), Value::String(converted));
        }
    }
}
