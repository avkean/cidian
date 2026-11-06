use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Word {
    #[serde(default)]
    pub id: u32,
    pub simplified: String,
    #[serde(default)]
    pub traditional: Vec<String>,
    pub pinyin: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub written: String,
    pub senses: Vec<Sense>,
    #[serde(default, skip_serializing)]
    pub search_glosses: Box<[String]>,
    #[serde(default, skip_serializing)]
    pub usual_for: std::collections::BTreeMap<String, u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classifiers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wiktionary_pages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hsk: Option<u8>,
    #[serde(default, skip_serializing)]
    pub frequency: f64,
    #[serde(default, skip_serializing)]
    pub reading_frequency: f64,
    #[serde(default, skip_serializing)]
    pub reading_corpus_frequency: f64,
    #[serde(default, skip_serializing)]
    pub corpus_frequency: u64,
    #[serde(default)]
    pub sources: Vec<Source>,
    /// display only, for readings with no written form: bit i is set when syllable i starts a new word
    #[serde(default, skip_deserializing, skip_serializing_if = "is_zero")]
    pub word_starts: u32,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    #[default]
    #[serde(rename = "CC")]
    CcCedict,
    #[serde(rename = "WIKT")]
    Wiktionary,
    #[serde(rename = "TATOEBA")]
    Tatoeba,
    #[serde(rename = "CORE")]
    Core,
}

impl Source {
    pub fn id(self) -> &'static str {
        match self {
            Self::CcCedict => "CC",
            Self::Wiktionary => "WIKT",
            Self::Tatoeba => "TATOEBA",
            Self::Core => "CORE",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Sense {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<Reference>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reference_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub source: Source,
    #[serde(skip)]
    pub usage: UsageFlags,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Reference {
    pub simplified: String,
    pub traditional: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UsageFlags {
    pub restricted: bool,
    /// distance from neutral use in half match classes (slang 1, obsolete 3, rude 5), so a whole meaning
    /// lands just after the plain whole, starting or mentioning matches
    pub caution: u8,
    pub proper: bool,
    pub formal: bool,
    pub regional: bool,
}

const REGIONAL_LABELS: &[&str] = &[
    "taiwan",
    "taiwanese",
    "mainland",
    "hong kong",
    "singapore",
    "malaysia",
];

pub(super) fn has_region(text: &str) -> bool {
    let text = format!(
        " {} ",
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    );
    REGIONAL_LABELS
        .iter()
        .any(|label| text.contains(&format!(" {label} ")))
}

const RESTRICTED_LABELS: [(&str, u8); 17] = [
    ("literary", 0),
    ("classical", 0),
    ("dialect", 0),
    ("rare", 0),
    ("historical", 0),
    ("bound form", 0),
    ("slang", 1),
    ("internet", 1),
    ("internet slang", 1),
    ("archaic", 3),
    ("dated", 3),
    ("obsolete", 3),
    ("old", 3),
    ("vulgar", 5),
    ("offensive", 5),
    ("derogatory", 5),
    ("pejorative", 5),
];

fn label_key(label: &str) -> String {
    let label = label.to_lowercase().replace('-', " ");
    if label == "bound" {
        "bound form".into()
    } else {
        label
    }
}

impl Sense {
    fn labelled_text(&self) -> String {
        let labels: Vec<&str> = self
            .tags
            .iter()
            .filter(|tag| caution(&label_key(tag)).is_some_and(|caution| caution > 0))
            .map(String::as_str)
            .collect();
        let text = short_meaning(&self.text);
        if labels.is_empty() {
            text.to_string()
        } else {
            format!("({}) {text}", labels.join(", "))
        }
    }

    pub fn normalize_usage(&mut self) {
        self.usage = UsageFlags::default();
        for label in self
            .tags
            .iter()
            .map(String::as_str)
            .chain(self.pos.as_deref())
        {
            let label = label_key(label);
            let label = label.as_str();
            add_label(&mut self.usage, label);
            self.usage.restricted |= label.ends_with("bound form") || label == "measure word";
            self.usage.proper |= label.contains("proper") || matches!(label, "surname" | "name");
            self.usage.formal |= label == "formal";
            self.usage.regional |= has_region(label);
        }
        // cc-cedict also puts labels after the meaning: "coffee (loanword) (old)"
        let mut text = self.text.trim_end();
        while let Some(rest) = text.strip_suffix(')')
            && let Some((before, label)) = rest.rsplit_once('(')
            && !before.trim().is_empty()
        {
            let label = label_key(label);
            text = before.trim_end();
            add_label(&mut self.usage, &label);
        }
    }
}

fn add_label(usage: &mut UsageFlags, label: &str) {
    if let Some(caution) = caution(label) {
        usage.restricted = true;
        usage.caution = usage.caution.max(caution);
    }
}

fn caution(label: &str) -> Option<u8> {
    RESTRICTED_LABELS
        .iter()
        .find(|&&(known, _)| known == label)
        .map(|&(_, caution)| caution)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Example {
    pub chinese: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pinyin: String,
    pub english: String,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sense_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribution: Vec<Credit>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Credit {
    pub id: u32,
    pub author: String,
}

impl Word {
    pub(super) fn searchable_senses(&self) -> impl Iterator<Item = (usize, &Sense)> {
        self.senses
            .iter()
            .enumerate()
            .filter(|(_, sense)| !sense.reference_only)
    }

    pub(super) fn attestation(&self) -> (bool, usize) {
        (
            self.sources
                .iter()
                .any(|source| matches!(source, Source::Core | Source::CcCedict)),
            self.sources.len(),
        )
    }

    pub fn traditional_form(&self) -> &str {
        self.traditional
            .first()
            .map_or(self.simplified.as_str(), String::as_str)
    }

    pub fn summary(&self) -> Summary {
        self.summary_leading(Lead::None)
    }

    pub fn summary_leading(&self, lead: Lead) -> Summary {
        let primary = self.senses.first().map(|sense| sense.source);
        let sense = match lead {
            Lead::Sense(at) => Some(usize::from(at)).filter(|&at| at < self.senses.len()),
            Lead::None => None,
        };
        let rest = |pointer: bool| {
            (0..self.senses.len()).filter(move |&at| {
                Some(self.senses[at].source) == primary
                    && Some(at) != sense
                    && self.senses[at].reference_only == pointer
            })
        };
        let gloss = sense
            .into_iter()
            .chain(rest(false))
            .chain(rest(true))
            .take(2)
            .map(|at| self.senses[at].labelled_text())
            .collect();
        Summary {
            id: self.id,
            simplified: self.simplified.clone(),
            traditional: self.traditional_form().to_string(),
            pinyin: self.pinyin.clone(),
            written: self.written.clone(),
            gloss,
            hsk: self.hsk,
            word_starts: self.word_starts,
        }
    }
}

fn short_meaning(text: &str) -> &str {
    let text = text.trim();
    let Some(rest) = text.strip_prefix('[') else {
        return text;
    };
    let Some(end) = rest.find(']') else {
        return text;
    };
    let after = rest[end + 1..].trim();
    if after.is_empty() {
        rest[..end].trim()
    } else {
        after
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub id: u32,
    pub simplified: String,
    pub traditional: String,
    pub pinyin: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub written: String,
    pub gloss: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hsk: Option<u8>,
    #[serde(skip_serializing_if = "is_zero")]
    pub word_starts: u32,
}

#[derive(Clone, Default, Deserialize, PartialEq, Eq, Hash)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
    #[serde(default)]
    pub word: String,
    #[serde(default)]
    pub reading: String,
    #[serde(default)]
    pub as_typed: bool,
}

impl SearchQuery {
    pub fn search_mode(&self) -> &str {
        match self.mode.as_str() {
            mode @ ("chinese" | "english" | "fulltext") => mode,
            _ => "auto",
        }
    }

    pub fn page_size(&self) -> usize {
        self.limit.unwrap_or(60).clamp(1, 100)
    }

    pub fn normalized(self) -> Self {
        Self {
            mode: self.search_mode().into(),
            limit: Some(self.page_size()),
            ..self
        }
    }
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub mode: String,
    pub total: usize,
    pub results: Vec<Summary>,
    pub exact: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corrected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<SearchSegment>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unfound: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pinyin: Vec<Summary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub english: Vec<Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capitals: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Clone, Serialize)]
pub struct SearchSegment {
    pub text: String,
    pub ids: Vec<u32>,
}

#[derive(Serialize)]
pub struct Reading {
    pub id: u32,
    pub pinyin: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub written: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub word_starts: u32,
}

#[derive(Serialize)]
pub struct SourceCount {
    pub id: String,
    pub words: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MatchKind {
    Exact,
    Prefix,
    Pattern,
    Contains,
    Usual,
    WholeGloss,
    LaterGloss,
    GlossHead,
    Mention,
    Substring,
    InMeanings,
    InExamples,
    Pinyin,
    PinyinPrefix,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum Lead {
    #[default]
    None,
    Sense(u16),
}

// Leads choose display text and must never break ranking ties.
impl PartialEq for Lead {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Lead {}

impl PartialOrd for Lead {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Lead {
    fn cmp(&self, _: &Self) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Relevance {
    pub kind: MatchKind,
    pub lead: Lead,
    pub sense_depth: u16,
    pub core_sense: bool,
    pub tail_pos: u8,
    pub tail_infinitive: bool,
    pub tail_explanation: bool,
    pub restricted: bool,
    pub sense_position: u8,
    pub qualifiers: u8,
    pub form_cost: u8,
    pub hidden: bool,
    pub regional: bool,
    pub proper: bool,
    pub caution: u8,
    pub named: bool,
}

impl Relevance {
    pub fn relevant(&self) -> bool {
        self.named || matches!(self.kind, MatchKind::Usual | MatchKind::Pinyin)
    }

    pub fn shown_whole(&self) -> bool {
        matches!(self.kind, MatchKind::WholeGloss | MatchKind::LaterGloss)
            && !self.hidden
            && !self.restricted
            && self.form_cost == 0
    }

    pub const fn new(kind: MatchKind) -> Self {
        Self {
            kind,
            lead: Lead::None,
            sense_depth: 0,
            core_sense: false,
            tail_pos: 0,
            tail_infinitive: false,
            tail_explanation: false,
            restricted: false,
            sense_position: 0,
            qualifiers: 0,
            form_cost: 0,
            hidden: false,
            regional: false,
            proper: false,
            caution: 0,
            named: false,
        }
    }
}
