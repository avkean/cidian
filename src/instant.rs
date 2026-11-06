//! the first page of the commonest searches in one file, built from the live index after startup
use crate::{AppState, content_hash, etag, search::SearchQuery};
use axum::{body::Bytes, http::HeaderValue};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

pub const RESULTS: usize = 10;
const ENGLISH_WORDS: usize = 2000;

pub struct Instant {
    pub path: String,
    pub body: Bytes,
    pub gzip: Bytes,
    pub etag: HeaderValue,
}

pub fn queries(state: &AppState) -> Vec<String> {
    let mut counted: Vec<(u64, &str)> = include_str!("../data/english-usage.tsv")
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let word = fields.next()?;
            (fields.next()? == "F").then_some(())?;
            Some((fields.next()?.parse().ok()?, word))
        })
        .collect();
    counted.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let mut queries: Vec<String> = counted
        .into_iter()
        .take(ENGLISH_WORDS)
        .map(|(_, word)| word.to_string())
        .collect();
    let mut syllables: Vec<&str> = state.dictionary.syllables().collect();
    syllables.sort_unstable();
    queries.extend(syllables.into_iter().map(String::from));
    let mut seen = HashSet::new();
    queries.retain(|query| seen.insert(query.clone()));
    queries
}

pub fn build(state: &AppState, queries: &[String]) -> Instant {
    let mut entries = Map::new();
    let mut searches = Map::new();
    for text in queries {
        let query = SearchQuery {
            q: text.clone(),
            limit: Some(RESULTS),
            ..SearchQuery::default()
        }
        .normalized();
        // Visitors take priority over building common searches.
        let permit = loop {
            match state.workers.try_acquire() {
                Ok(permit) => break permit,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        };
        let answer = state.answer(&query);
        drop(permit);
        let Value::Object(mut answer) = answer else {
            unreachable!("Search answers are objects")
        };
        for field in ["query", "mode", "exact"] {
            answer.remove(field);
        }
        for list in ["results", "pinyin", "english"] {
            let Some(Value::Array(summaries)) = answer.get_mut(list) else {
                continue;
            };
            for summary in summaries.iter_mut() {
                let id = summary["id"].clone();
                let field = |name: &str| summary.get(name).cloned().unwrap_or(Value::Null);
                let traditional = if summary["traditional"] == summary["simplified"] {
                    json!("")
                } else {
                    field("traditional")
                };
                let mut entry = vec![
                    field("simplified"),
                    traditional,
                    field("pinyin"),
                    field("gloss"),
                ];
                let extra: Map<String, Value> = ["traditional_text", "written", "word_starts"]
                    .into_iter()
                    .filter_map(|name| Some((name.to_string(), summary.get(name)?.clone())))
                    .collect();
                if !extra.is_empty() {
                    entry.push(Value::Object(extra));
                }
                let entry = Value::Array(entry);
                // One word can have different leading meanings, so each distinct row needs a key.
                let key = |n| match n {
                    0 => id.to_string(),
                    n => format!("{id}~{n}"),
                };
                let n = (0..)
                    .find(|&n| entries.get(&key(n)).is_none_or(|known| *known == entry))
                    .expect("some key is free");
                entries.insert(key(n), entry);
                *summary = if n == 0 { id } else { json!(key(n)) };
            }
        }
        searches.insert(text.clone(), Value::Object(answer));
    }
    let body = serde_json::to_vec(&json!({ "entries": entries, "searches": searches }))
        .expect("Instant answers are serializable");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut encoder, &body).expect("Writing to memory works");
    let gzip = encoder.finish().expect("Writing to memory works");
    Instant {
        path: format!(
            "/api/instant/{}-{:016x}",
            state.revision,
            content_hash(&body)
        ),
        etag: etag(&body),
        body: Bytes::from(body),
        gzip: Bytes::from(gzip),
    }
}
