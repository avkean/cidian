use super::{SearchIndex, fold_english, lexicon, tokens};

pub(super) struct LexicalQuery<'a> {
    pub(super) marks: Option<(bool, &'a [(u8, u32)])>,
    pub(super) support: Vec<(u32, u8)>,
}

impl SearchIndex {
    pub(super) fn lexical_query(&self, text: &str) -> LexicalQuery<'_> {
        let keys = self.usual_queries(text);
        LexicalQuery {
            marks: self.marks_for(text, &keys),
            support: self.support_for(&keys),
        }
    }

    pub(super) fn usual_queries(&self, text: &str) -> Vec<String> {
        let normalized = tokens(&fold_english(text)).collect::<Vec<_>>().join(" ");
        if self.is_abbreviation(text) {
            return vec![normalized];
        }
        let mut queries = vec![String::new()];
        let preserve_noun = normalized.split_whitespace().count() == 1;
        for word in normalized.split_whitespace() {
            let forms = std::iter::once(word.to_string())
                .chain(lexicon::lexicon().lemmas(word, preserve_noun));
            queries = forms
                .flat_map(|form| {
                    queries.iter().map(move |prefix| {
                        if prefix.is_empty() {
                            form.clone()
                        } else {
                            format!("{prefix} {form}")
                        }
                    })
                })
                .take(64)
                .collect();
        }
        let without_to: Vec<_> = queries
            .iter()
            .filter_map(|q| q.strip_prefix("to ").map(str::to_string))
            .collect();
        queries.extend(without_to);
        queries
    }

    // Verb inflections prefer the verb's marks: "led" takes "to lead" before the metal "lead".
    fn marks_for(&self, text: &str, queries: &[String]) -> Option<(bool, &[(u8, u32)])> {
        let typed = queries.first()?;
        let lexicon = lexicon::lexicon();
        // "IT" and "US" aren't "it" and "us"
        if self.is_abbreviation(text) && typed.split(' ').any(|word| lexicon.knows(word)) {
            return None;
        }
        let bare = |query: &'_ str| query.strip_prefix("to ").unwrap_or(query).to_string();
        let words = bare(typed);
        let same = |query: &String| {
            let query = bare(query);
            query.split(' ').count() == words.split(' ').count()
                && query
                    .split(' ')
                    .zip(words.split(' '))
                    .all(|(a, b)| lexicon.same_spelling(b, a))
        };
        let own = [typed.clone(), words.clone()]
            .into_iter()
            .chain(queries.iter().filter(|query| same(query)).cloned());
        if let Some(ids) = own.filter_map(|query| self.usual.get(&query)).next() {
            return Some((true, ids));
        }
        let verbs = if !typed.contains(' ') && lexicon.verb_inflection(typed) {
            lexicon.lemmas(typed, true)
        } else {
            Vec::new()
        };
        let verbs = verbs.into_iter().map(|lemma| format!("to {lemma}"));
        verbs
            .chain(queries.iter().skip(1).cloned())
            .find_map(|query| self.usual.get(&query))
            .map(|ids| (false, ids.as_slice()))
    }
}
