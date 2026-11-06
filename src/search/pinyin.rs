use super::is_hanzi;
use unicode_normalization::UnicodeNormalization;

/// a breve (ĭ, ă) is the usual keyboard and font stand-in for a caron
pub(super) fn tone_mark(c: char) -> Option<u8> {
    match c {
        '\u{0304}' => Some(1),
        '\u{0301}' => Some(2),
        '\u{030c}' | '\u{0306}' => Some(3),
        '\u{0300}' => Some(4),
        _ => None,
    }
}

/// indexes spell ü as u, an explicit v or ü is checked separately
fn base_letter(c: char) -> char {
    if c == 'v' { 'u' } else { c }
}

fn wildcard(c: char) -> Option<Pattern> {
    match c {
        '?' | '@' => Some(Pattern::Wild(1, 1)),
        '*' | '$' => Some(Pattern::Wild(0, 3)),
        _ => None,
    }
}

pub(super) fn is_wildcard(c: char) -> bool {
    wildcard(c).is_some()
}

pub(super) fn has_wildcard(text: &str) -> bool {
    text.chars().any(is_wildcard)
}

#[derive(Default, Debug)]
pub(super) struct Pinyin {
    pub(super) letters: String,
    pub(super) tones: Vec<(usize, u8)>,
    pub(super) syllables: Vec<(String, u8)>,
    pub(super) letter_tones: Vec<u8>,
    pub(super) umlauts: Vec<usize>,
    pub(super) boundaries: Vec<usize>,
    pub(super) terminal_tone: bool,
}

impl Pinyin {
    fn push_letter(&mut self, c: char) {
        if c == 'v' {
            self.umlauts.push(self.letters.len());
        }
        self.letters.push(base_letter(c));
    }

    pub(super) fn parse(text: &str) -> Self {
        let mut result = Self {
            terminal_tone: text.trim().ends_with(['0', '1', '2', '3', '4', '5']),
            ..Self::default()
        };
        let mut word = String::new();
        let mut tone = 5;
        let flush = |result: &mut Self, word: &mut String, tone: &mut u8| {
            if !word.is_empty() {
                result
                    .letter_tones
                    .extend(std::iter::repeat_n(*tone, word.len()));
                result.syllables.push((std::mem::take(word), *tone));
                result.boundaries.push(result.letters.len());
            }
            *tone = 5;
        };
        for c in text.to_lowercase().nfd() {
            match c {
                'a'..='z' => {
                    word.push(c);
                    result.push_letter(c);
                }
                '\u{0308}' | ':' if word.ends_with('u') => {
                    // unmarked u stays forgiving, an explicit ü/v/u: is honoured
                    result.umlauts.push(result.letters.len() - 1);
                    word.pop();
                    word.push('v');
                }
                _ if let Some(mark) = tone_mark(c) => {
                    tone = mark;
                    if !result.letters.is_empty() {
                        result.tones.push((result.letters.len() - 1, tone));
                    }
                }
                '0'..='5' => {
                    tone = if c == '0' { 5 } else { c as u8 - b'0' };
                    if !word.is_empty() {
                        result.tones.push((result.letters.len() - 1, tone));
                    }
                    flush(&mut result, &mut word, &mut tone);
                }
                _ => flush(&mut result, &mut word, &mut tone),
            }
        }
        flush(&mut result, &mut word, &mut tone);
        result
    }

    /// typed breaks (spaces, apostrophes, tone numbers) must fall between syllables, but an unbroken run may span
    /// several ("zhongguo2"); `citation` says whether a neutral syllable inside the word takes a full tone
    pub(super) fn matches_prefix(
        &self,
        query: &Self,
        citation: impl Fn(usize, u8) -> bool,
    ) -> bool {
        let typed = query.letters.len();
        self.letters.starts_with(&query.letters)
            && query
                .umlauts
                .iter()
                .all(|index| self.umlauts.contains(index))
            && query
                .boundaries
                .iter()
                .all(|end| *end >= typed || self.boundaries.contains(end))
            && (!query.terminal_tone || self.boundaries.contains(&typed))
            && query.tones.iter().all(|&(index, tone)| {
                let Some(&own) = self.letter_tones.get(index) else {
                    return false;
                };
                let syllable = self
                    .boundaries
                    .iter()
                    .take_while(|end| **end <= index)
                    .count();
                let start = syllable.checked_sub(1).map_or(0, |i| self.boundaries[i]);
                // erhua r takes the tone of the syllable it joins (nar3)
                if start > 0
                    && self.boundaries.contains(&(start + 1))
                    && self.letters[start..].starts_with('r')
                {
                    return own == tone || self.letter_tones[start - 1] == tone;
                }
                own == tone || (own == 5 && start > 0 && citation(syllable, tone))
            })
    }
}

pub(super) enum Pattern {
    Han(char),
    Pinyin(Pinyin),
    Wild(usize, usize),
}

// One unit per index key, candidate, matcher state or pinyin syllable.
pub(super) const PATTERN_WORK: usize = 240_000;

pub(super) struct PatternBudget {
    pub(super) remaining: usize,
    pub(super) exhausted: bool,
}

impl PatternBudget {
    pub(super) fn take(&mut self, cost: usize) -> bool {
        if let Some(remaining) = self.remaining.checked_sub(cost) {
            self.remaining = remaining;
            true
        } else {
            self.exhausted = true;
            false
        }
    }
}

pub(super) fn pattern(text: &str) -> Vec<Pattern> {
    let mut tokens = Vec::new();
    let mut latin = String::new();
    let flush = |tokens: &mut Vec<Pattern>, latin: &mut String| {
        if !latin.is_empty() {
            tokens.push(Pattern::Pinyin(Pinyin::parse(latin)));
            latin.clear();
        }
    };
    for c in text.chars() {
        let token = wildcard(c).or_else(|| is_hanzi(c).then_some(Pattern::Han(c)));
        if let Some(token) = token {
            flush(&mut tokens, &mut latin);
            if let (Some(Pattern::Wild(min, max)), Pattern::Wild(next_min, next_max)) =
                (tokens.last_mut(), &token)
            {
                // Adjacent bounded wildcards cover the sum of their ranges.
                *min += next_min;
                *max += next_max;
            } else {
                tokens.push(token);
            }
        } else {
            latin.push(c);
        }
    }
    flush(&mut tokens, &mut latin);
    tokens
}

pub(super) fn mixed_matches(
    pattern: &[Pattern],
    headword: &str,
    pronunciation: &Pinyin,
    budget: &mut PatternBudget,
) -> bool {
    let chars: Vec<char> = headword.chars().collect();
    fn step(
        pattern: &[Pattern],
        chars: &[char],
        syllables: &[(String, u8)],
        position: usize,
        failed: &mut [bool],
        budget: &mut PatternBudget,
    ) -> bool {
        if !budget.take(1) {
            return false;
        }
        let Some((first, rest)) = pattern.split_first() else {
            return true;
        };
        if position > chars.len() {
            return false;
        }
        let state = pattern.len() * (chars.len() + 1) + position;
        if failed[state] {
            return false;
        }
        let matched = match first {
            Pattern::Han(c) => {
                chars.get(position) == Some(c)
                    && step(rest, chars, syllables, position + 1, failed, budget)
            }
            Pattern::Wild(min, max) => {
                let mut matched = false;
                for count in *min..=(*max).min(chars.len() - position) {
                    if budget.exhausted {
                        break;
                    }
                    if step(rest, chars, syllables, position + count, failed, budget) {
                        matched = true;
                        break;
                    }
                }
                matched
            }
            Pattern::Pinyin(query) => {
                if query.letters.is_empty() || syllables.len() != chars.len() {
                    return false;
                }
                let mut combined = Pinyin::default();
                for (offset, (letters, tone)) in syllables[position..].iter().enumerate() {
                    if !budget.take(1) {
                        return false;
                    }
                    for c in letters.chars() {
                        combined.push_letter(c);
                    }
                    combined.boundaries.push(combined.letters.len());
                    combined
                        .letter_tones
                        .extend(std::iter::repeat_n(*tone, letters.len()));
                    let matches = if rest.is_empty() {
                        combined.matches_prefix(query, |_, _| true)
                    } else {
                        combined.letters == query.letters
                            && combined.matches_prefix(query, |_, _| true)
                    };
                    if matches
                        && step(
                            rest,
                            chars,
                            syllables,
                            position + offset + 1,
                            failed,
                            budget,
                        )
                    {
                        return true;
                    }
                    if combined.letters.len() >= query.letters.len() {
                        break;
                    }
                }
                false
            }
        };
        if !matched {
            failed[state] = true;
        }
        matched
    }
    step(
        pattern,
        &chars,
        &pronunciation.syllables,
        0,
        &mut vec![false; (pattern.len() + 1) * (chars.len() + 1)],
        budget,
    )
}

pub(super) fn letters(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .nfd()
        .filter(char::is_ascii_alphabetic)
        .map(base_letter)
        .collect()
}
