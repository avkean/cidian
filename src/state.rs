use crate::{
    cache::{ASSETS, SearchCache, fingerprinted},
    handwriting::HandwritingIndex,
    instant,
    pages::{DICTIONARY_BODY, DICTIONARY_PAGE, SOURCE_NAMES, page_html, sources_page},
    requests::{Computations, Stats, Uploads, Visitors},
    search::{SearchIndex, SearchQuery},
    traditional,
};
use axum::body::Bytes;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};
use tokio::sync::Semaphore;

pub(crate) struct AppState {
    pub(crate) dictionary: SearchIndex,
    pub(crate) handwriting: Option<HandwritingIndex>,
    pub(crate) traditional: traditional::Traditional,
    pub(crate) licenses: BTreeMap<String, String>,
    pub(crate) sources_page: String,
    pub(crate) info: Value,
    pub(crate) home_page: String,
    pub(crate) dictionary_page: String,
    pub(crate) not_found_page: String,
    pub(crate) instant: OnceLock<instant::Instant>,
    pub(crate) revision: String,
    pub(crate) workers: Arc<Semaphore>,
    pub(crate) computations: Arc<Computations>,
    pub(crate) uploads: Arc<Uploads>,
    pub(crate) searches: Mutex<SearchCache>,
    pub(crate) stats: Stats,
    pub(crate) visitors: Visitors,
}

impl AppState {
    pub(crate) fn new(
        dictionary: SearchIndex,
        handwriting: Option<HandwritingIndex>,
        licenses: BTreeMap<String, String>,
        workers: usize,
        feedback_url: Option<&str>,
    ) -> Self {
        let sources: Vec<Value> = dictionary
            .source_counts()
            .into_iter()
            .filter_map(|source| {
                let name = SOURCE_NAMES.iter().find(|(id, _)| *id == source.id)?.1;
                Some(json!({ "id": source.id, "name": name, "words": source.words }))
            })
            .collect();
        Self {
            info: json!({ "words": dictionary.words.len(), "sources": sources }),
            traditional: traditional::Traditional::new(&dictionary.words),
            sources_page: fingerprinted(&sources_page(&licenses), &ASSETS),
            home_page: page_html(
                &DICTIONARY_PAGE.replace(DICTIONARY_BODY, "<body class=\"home\""),
                feedback_url,
                dictionary.words.len(),
            ),
            dictionary_page: page_html(DICTIONARY_PAGE, feedback_url, dictionary.words.len()),
            not_found_page: page_html(
                include_str!("../web/not-found.html"),
                feedback_url,
                dictionary.words.len(),
            ),
            instant: OnceLock::new(),
            revision: String::new(),
            dictionary,
            handwriting,
            licenses,
            workers: Arc::new(Semaphore::new(workers)),
            computations: Arc::new(Computations::default()),
            uploads: Arc::new(Uploads::default()),
            searches: Mutex::new(SearchCache::default()),
            stats: Stats::default(),
            visitors: Visitors::default(),
        }
    }

    pub(crate) fn answer(&self, query: &SearchQuery) -> Value {
        self.traditional.display(&self.dictionary.search(query))
    }

    /// Cached answers cannot go stale because the dictionary is immutable while running.
    pub(crate) fn search(&self, query: SearchQuery) -> Bytes {
        let mut answer = self.answer(&query);
        let body = Bytes::from(serde_json::to_vec(&answer).expect("Searches are serializable"));
        let key = self.dictionary.cache_query(&query);
        let cached = if key.q == query.q {
            body.clone()
        } else {
            answer["query"] = json!(key.q);
            Bytes::from(serde_json::to_vec(&answer).expect("Searches are serializable"))
        };
        self.searches.lock().unwrap().insert(key, cached);
        body
    }

    pub(crate) fn cached_search(&self, query: &SearchQuery) -> Option<Bytes> {
        let key = self.dictionary.cache_query(query);
        let body = self.searches.lock().unwrap().get(&key)?;
        if key.q == query.q {
            return Some(body);
        }
        // The answer is shared, but each visitor still sees the query they typed.
        let mut answer: Value = serde_json::from_slice(&body).expect("Cached search is JSON");
        answer["query"] = json!(query.q);
        Some(Bytes::from(
            serde_json::to_vec(&answer).expect("Searches are serializable"),
        ))
    }
}
