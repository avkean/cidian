use super::lexicon::lexicon;

const MAX_LENGTH: usize = 24;

#[derive(Default)]
pub(super) struct Speller {
    text: String,
    words: Vec<Candidate>,
    by_length: Vec<usize>,
}

struct Candidate {
    start: u32,
    length: u8,
    letters: u32,
    count: u32,
}

fn letter_set(word: &str) -> u32 {
    word.bytes().fold(0, |set, b| set | 1 << (b - b'a'))
}

fn plain(word: &str) -> bool {
    (2..=MAX_LENGTH).contains(&word.len()) && word.bytes().all(|b| b.is_ascii_lowercase())
}

impl Speller {
    pub(super) fn new<'a>(vocabulary: impl Iterator<Item = &'a str>) -> Self {
        let mut speller = Self::default();
        let mut words: Vec<&str> = vocabulary.filter(|word| plain(word)).collect();
        words.sort_unstable_by_key(|word| (word.len(), *word));
        words.dedup();
        speller.by_length = (0..=MAX_LENGTH + 1)
            .map(|length| words.partition_point(|word| word.len() < length))
            .collect();
        speller.words = words
            .iter()
            .map(|word| {
                let start = speller.text.len() as u32;
                speller.text.push_str(word);
                Candidate {
                    start,
                    length: word.len() as u8,
                    letters: letter_set(word),
                    count: lexicon().count(word),
                }
            })
            .collect();
        speller.text.shrink_to_fit();
        speller
    }

    fn word(&self, candidate: &Candidate) -> &str {
        let start = candidate.start as usize;
        &self.text[start..start + candidate.length as usize]
    }

    // First-letter changes are less likely typing slips, so they cost an extra edit.
    pub(super) fn suggest(&self, typed: &str) -> Option<&str> {
        if self.words.is_empty() || !plain(typed) || typed.len() < 3 || lexicon().is_word(typed) {
            return None;
        }
        let edits = if typed.len() <= 4 { 1 } else { 2 };
        let letters = letter_set(typed);
        let range = self.by_length[typed.len() - edits]
            ..self.by_length[(typed.len() + edits + 1).min(MAX_LENGTH + 1)];
        let mut best: Option<(usize, std::cmp::Reverse<u32>, &str)> = None;
        for candidate in &self.words[range] {
            // each edit adds or removes at most two letters from the set
            if (candidate.letters ^ letters).count_ones() as usize > 2 * edits {
                continue;
            }
            let word = self.word(candidate);
            let limit = if word.as_bytes()[0] == typed.as_bytes()[0] {
                edits
            } else {
                edits - 1
            };
            // one moved letter is one slip ("tounge" is "tongue", not "tough")
            let found = if moved(typed.as_bytes(), word.as_bytes()) {
                (limit >= 1).then_some(1)
            } else {
                distance(typed.as_bytes(), word.as_bytes(), limit)
            };
            if let Some(distance) = found {
                let distance = distance + edits - limit;
                let key = (distance, std::cmp::Reverse(candidate.count), word);
                if best.is_none_or(|best| key < best) {
                    best = Some(key);
                }
            }
        }
        best.map(|(_, _, word)| word)
    }
}

fn moved(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let end = a[start..]
        .iter()
        .rev()
        .zip(b[start..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a, b) = (&a[start..a.len() - end], &b[start..b.len() - end]);
    a.len() >= 2
        && (a[1..] == b[..b.len() - 1] && a[0] == b[b.len() - 1]
            || a[..a.len() - 1] == b[1..] && a[a.len() - 1] == b[0])
}

fn distance(a: &[u8], b: &[u8], limit: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > limit || a.len() > MAX_LENGTH || b.len() > MAX_LENGTH {
        return None;
    }
    let mut before = [0u8; MAX_LENGTH + 1];
    let mut previous: [u8; MAX_LENGTH + 1] = std::array::from_fn(|j| j as u8);
    let mut current = [0u8; MAX_LENGTH + 1];
    for i in 1..=a.len() {
        current[0] = i as u8;
        let mut lowest = current[0];
        for j in 1..=b.len() {
            let mut value = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + u8::from(a[i - 1] != b[j - 1]));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                value = value.min(before[j - 2] + 1);
            }
            current[j] = value;
            lowest = lowest.min(value);
        }
        if usize::from(lowest) > limit {
            return None;
        }
        before = previous;
        previous = current;
    }
    let distance = usize::from(previous[b.len()]);
    (distance <= limit).then_some(distance)
}
