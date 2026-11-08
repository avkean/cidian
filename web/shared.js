const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
const escapeHTML = value => String(value ?? '').replace(/[&<>"']/g, char => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[char]));
const icon = name => `<svg aria-hidden="true"><use href="#i-${name}"/></svg>`;
const HAN = /\p{Script=Han}/u;
const reducedMotion = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
const mobileQuery = matchMedia('(max-width: 680px)');
const mobileView = () => mobileQuery.matches;
const REQUEST_TIMEOUT = 15000;
const RETRY_LIMIT = 5;
const LOADING_DELAY = 180;
const PENDING_REQUEST_LIMIT = 4;
const PREFETCH_REQUEST_LIMIT = 2;
const SEARCH_PAGE_SIZE = 30;
const SEARCH_CACHE_CAPACITY = 64;
const INSTANT_RETRY_DELAYS = [30000, 60000, 120000];
const LOOKUP_RESULT_LIMIT = 8;
const LOOKUP_MAX_HEIGHT = 360;
const SHEET_CLOSE_DISTANCE = 80;
const SHEET_FLICK = .5;
const ENTRY_CACHE_CAPACITY = 48;
const CHARACTER_CACHE_CAPACITY = 24;
const LONG_HEADWORD = 6;
// characters prefetched for one word; the tab fetches the rest
const CHARACTER_PREFETCH = 12;
const CHARACTER_SENSES = 3;
const SOURCE_NAMES = {CC: 'CC-CEDICT', WIKT: 'Wiktionary', TATOEBA: 'Tatoeba', CORE: 'avkean 词典'};

function readStored(key, fallback) {
  try {
    return JSON.parse(localStorage.getItem(key)) ?? fallback;
  } catch {
    return fallback;
  }
}

function writeStored(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {}
}

// Old IDs and late answers become invalid after a rebuild.
const answerRevisions = new WeakMap();
const revision = {current: document.body.dataset.revision || null, previous: new Set(), changed: () => {}, of: data => answerRevisions.get(data)};

async function acceptRevision(value, signal) {
  if (!value || value === revision.current) return true;
  const before = revision.current;
  if (revision.previous.has(value)) {
    // A late answer is stale unless the server confirms a rollback.
    const info = await fetch('/api/info', {cache: 'no-store', signal});
    if (!info.ok || info.headers.get('X-Dictionary-Revision') !== value || signal.aborted) return false;
    if (revision.current !== before) return value === revision.current;
    revision.previous.delete(value);
  }
  revision.current = value;
  if (before) {
    revision.previous.add(before);
    revision.changed();
  }
  return true;
}

// every request in flight, so a page about to load fresh can drop them first
const inflight = new Set();
const abortAll = () => { for (const controller of inflight) controller.abort(); };

async function fetchJSON(url, {retry = true, ...options} = {}) {
  const controller = new AbortController();
  inflight.add(controller);
  const caller = options.signal;
  const cancel = () => controller.abort();
  if (caller?.aborted) cancel();
  else caller?.addEventListener('abort', cancel, {once: true});
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, REQUEST_TIMEOUT);
  try {
    const request = async () => {
      const response = await fetch(url, {...options, signal: controller.signal});
      if (!await acceptRevision(response.headers.get('X-Dictionary-Revision'), controller.signal)) throw new DOMException('Answer from an older dictionary', 'AbortError');
      return response;
    };
    let response = await request();
    if (response.status === 503 && retry) {
      const wait = Math.min(Number(response.headers.get('Retry-After')) || 1, RETRY_LIMIT);
      await new Promise((resolve, reject) => {
        const waiting = setTimeout(resolve, wait * 1000);
        controller.signal.addEventListener('abort', () => {
          clearTimeout(waiting);
          reject(new DOMException('Aborted', 'AbortError'));
        }, {once: true});
      });
      response = await request();
    }
    const answerRevision = response.headers.get('X-Dictionary-Revision');
    let data;
    try {
      data = await response.json();
    } catch (error) {
      if (response.ok) throw error;
    }
    if (answerRevision && answerRevision !== revision.current) throw new DOMException('Answer from an older dictionary', 'AbortError');
    if (!response.ok) {
      const message = data?.error || 'The dictionary ran into a problem. Try again.';
      throw Object.assign(new Error(message), {status: response.status});
    }
    if (data && typeof data === 'object') answerRevisions.set(data, answerRevision || revision.current);
    return data;
  } catch (error) {
    if (timedOut && !caller?.aborted) {
      const timeout = new Error('The dictionary took too long to respond. Try again.');
      timeout.name = 'TimeoutError';
      throw timeout;
    }
    if (error instanceof TypeError) throw new Error('The dictionary could not be reached. Check your connection and try again.');
    throw error;
  } finally {
    clearTimeout(timer);
    caller?.removeEventListener('abort', cancel);
    inflight.delete(controller);
  }
}

const loadingTargets = new WeakMap();
const visibleLoads = new Set();
const loadingStatus = document.createElement('span');
loadingStatus.className = 'visually-hidden';
loadingStatus.setAttribute('role', 'status');
loadingStatus.setAttribute('aria-live', 'polite');
loadingStatus.setAttribute('aria-atomic', 'true');
document.body.append(loadingStatus);

function beginLoading(target, message, signal, lock = true) {
  loadingTargets.get(target)?.();
  let finished = false;
  target.setAttribute('aria-busy', 'true');
  // old results must not stay clickable while new ones load
  if (lock) target.inert = true;
  const timer = setTimeout(() => {
    visibleLoads.add(finish);
    $('.loading-bar').classList.add('active');
    loadingStatus.textContent = message;
  }, LOADING_DELAY);
  const cancel = () => finish();
  function finish(message = '') {
    if (finished) return;
    finished = true;
    clearTimeout(timer);
    signal?.removeEventListener('abort', cancel);
    visibleLoads.delete(finish);
    loadingTargets.delete(target);
    target.setAttribute('aria-busy', 'false');
    if (lock) target.inert = false;
    $('.loading-bar').classList.toggle('active', visibleLoads.size > 0);
    if (message || !visibleLoads.size) loadingStatus.textContent = message;
  }
  loadingTargets.set(target, finish);
  if (signal?.aborted) finish();
  else signal?.addEventListener('abort', cancel, {once: true});
  return finish;
}

function resourceCache(url, capacity, {selected = () => null, pending = PENDING_REQUEST_LIMIT, prefetches = PREFETCH_REQUEST_LIMIT} = {}) {
  const values = new Map();
  const requests = new Map();
  // Do not cache answers from before a clear.
  let generation = 0;
  // A failed prefetch must not prevent a caller's retry.
  function get(key, prefetch = false) {
    if (values.has(key)) {
      const value = values.get(key);
      values.delete(key);
      values.set(key, value);
      return value;
    }
    if (requests.has(key)) {
      const request = requests.get(key);
      return request.prefetch && !prefetch ? request.promise.catch(error => {
        if (error.name === 'AbortError') throw error;
        return get(key);
      }) : request.promise;
    }
    if (requests.size >= pending) {
      const oldest = [...requests.keys()].find(item => item !== selected());
      requests.get(oldest).controller.abort();
      requests.delete(oldest);
    }
    const controller = new AbortController();
    const started = generation;
    const promise = fetchJSON(url(key), {signal: controller.signal, retry: !prefetch}).then(value => {
      if (!controller.signal.aborted && started === generation) {
        values.set(key, value);
        if (values.size > capacity) values.delete(values.keys().next().value);
      }
      return value;
    }).finally(() => {
      if (requests.get(key)?.controller === controller) requests.delete(key);
    });
    requests.set(key, {controller, promise, prefetch});
    return promise;
  }
  return {
    get,
    peek: key => values.get(key),
    abort(key) {
      requests.get(key)?.controller.abort();
      requests.delete(key);
    },
    clear() {
      generation++;
      values.clear();
      requests.clear();
    },
    prefetch(key) {
      if (!navigator.connection?.saveData && !values.has(key) && requests.size < prefetches) {
        Promise.resolve(get(key, true)).catch(() => {});
      }
    },
  };
}

function loadPreferences() {
  const stored = readStored('cidian.preferences', {});
  return {
    traditional: stored?.traditional === true,
    pinyin: stored?.pinyin === 'numbered' ? 'numbered' : 'marked',
    theme: window.validTheme(stored?.theme) ? stored.theme : 'system',
    examplePinyin: stored?.examplePinyin === true,
  };
}

const preferences = loadPreferences();

function applyTheme() {
  if (preferences.theme === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = preferences.theme;
}

applyTheme();

// colours switch at once, not control by control
function holdTransitions() {
  const root = document.documentElement;
  root.classList.add('theme-change');
  requestAnimationFrame(() => requestAnimationFrame(() => root.classList.remove('theme-change')));
}

matchMedia('(prefers-color-scheme: dark)').addEventListener('change', holdTransitions);

const presence = new WeakMap();
const seconds = value => parseFloat(value) * (value.trim().endsWith('ms') ? 1 : 1000);

function motion(element, keyframes, out = false) {
  const root = getComputedStyle(document.documentElement);
  return element.animate(keyframes, {
    duration: seconds(root.getPropertyValue(out ? '--move-out' : '--move')),
    easing: root.getPropertyValue('--ease-move').trim(),
    fill: 'forwards',
  });
}

// Interrupted motion resumes from the current pose, including a held drag.
function present(element, show) {
  const running = presence.get(element);
  const dragged = Boolean(element.style.transform);
  if (running ? running.show === show : element.hidden === !show && !dragged) return;
  const style = getComputedStyle(element);
  const away = {opacity: 0, transform: style.getPropertyValue('--away').trim() || 'none'};
  const here = {opacity: 1, transform: 'none'};
  const from = running || dragged ? {opacity: style.opacity, transform: style.transform} : show ? away : here;
  running?.animation.cancel();
  presence.delete(element);
  element.style.transform = '';
  element.hidden = !show;
  element.inert = !show;
  if (reducedMotion()) return element.classList.remove('leaving');
  element.classList.toggle('leaving', !show);
  const animation = motion(element, [from, show ? here : away], !show);
  const run = {show, animation};
  presence.set(element, run);
  animation.finished.then(() => {
    if (presence.get(element) !== run) return;
    presence.delete(element);
    element.classList.remove('leaving');
    animation.cancel();
  }, () => {});
}

function hold(element) {
  const running = presence.get(element);
  if (!running) return;
  element.style.transform = getComputedStyle(element).transform;
  running.animation.cancel();
  presence.delete(element);
}
const SEARCH_MODES = ['auto', 'chinese', 'english', 'fulltext'];
const TABS = ['meanings', 'words', 'characters'];
const content = $('#entry-content');
const query = $('#query');
const popup = $('#lookup-popover');

const state = {
  latestSearch: null,
  editingSearch: false,
  // Scope from a link or address lasts one search; the settings choice persists.
  preference: 'auto',
  scope: null,
  served: null,
  query: '',
  results: [],
  segments: [],
  pinyin: [],
  english: [],
  capitals: '',
  unfound: [],
  total: 0,
  truncated: false,
  resultsRevision: null,
  selected: null,
  summary: null,
  detail: null,
  tab: 'meanings',
  composing: false,
  requests: {},
  peek: {anchor: null, returnTo: null, keyboard: false, bounds: null},
};

// Keep the same search box to retain focus and the phone keyboard.
const isHome = () => document.body.classList.contains('home');

function syncMobilePanes() {
  const detailOpen = mobileView() && document.body.classList.contains('detail-open');
  $('.results-pane').inert = detailOpen;
  $('#entry-pane').inert = mobileView() && !detailOpen;
}

function setDetailOpen(open) {
  document.body.classList.toggle('detail-open', open);
  syncMobilePanes();
}

function abortPending(...kinds) {
  for (const kind of kinds) {
    state.requests[kind]?.abort();
    delete state.requests[kind];
  }
}

function begin(kind) {
  abortPending(kind);
  const controller = new AbortController();
  state.requests[kind] = controller;
  return controller;
}

const current = (kind, controller) => state.requests[kind] === controller && !controller.signal.aborted;

function toast(message) {
  const element = $('#toast');
  element.textContent = message;
  element.classList.add('visible');
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => element.classList.remove('visible'), 2600);
}

// Remove the class so a parent cannot re-animate later content.
function restartAnimation(element, className) {
  if (reducedMotion()) return;
  element.classList.remove(className);
  requestAnimationFrame(() => {
    if (!element.isConnected) return;
    element.classList.add(className);
    const done = event => {
      if (event.target !== element && event.target.parentElement !== element) return;
      element.classList.remove(className);
      element.removeEventListener('animationend', done);
    };
    element.addEventListener('animationend', done);
  });
}
const searchMode = () => state.scope || state.preference;
// The server decides # searches, overriding the saved mode.
const shown = () => state.served === 'fulltext' ? {preference: 'fulltext', scope: null} : state;
const openDisclosures = () => $$('details', content).map(details => details.open && !details.classList.contains('closing'));

// CSS chooses which pane shows these shared states.
const stateHTML = (heading, glyph, title, text, action = '') => `<div class="empty-entry"><div><span lang="zh" aria-hidden="true">${glyph}</span><${heading}>${title}</${heading}><p>${text}</p>${action}</div></div>`;
const listStateHTML = (...message) => `<div class="list-state">${stateHTML('h2', ...message)}</div>`;

function entryState(...message) {
  content.setAttribute('aria-busy', 'false');
  content.innerHTML = stateHTML('h1', ...message);
}

// A failed retry replaces its button, so restore focus to the new one.
const retryBusy = kind => Boolean($(`[data-retry-${kind}][aria-busy="true"]`));
function retryFocus(kind) {
  const button = $$(`[data-retry-${kind}]`).find(item => item.getClientRects().length);
  button?.focus({preventScroll: true});
}
const searches = resourceCache(key => `/api/search?${key}`, SEARCH_CACHE_CAPACITY, {selected: () => state.latestSearch});
const entries = resourceCache(id => `/api/entry/${id}`, ENTRY_CACHE_CAPACITY, {selected: () => state.selected});
// a word's characters load as one group, so none is cut short by another
const characters = resourceCache(char => `/api/character/${encodeURIComponent(char)}`, CHARACTER_CACHE_CAPACITY, {pending: Infinity, prefetches: CHARACTER_PREFETCH});

// Warm common searches while idle unless data saving is on.
let instant = null;
let instantURL = document.body.dataset.instant || null;
let instantGeneration = 0;

async function loadInstant(retries = INSTANT_RETRY_DELAYS) {
  if (navigator.connection?.saveData || instant) return;
  const generation = ++instantGeneration;
  try {
    if (!instantURL) {
      const info = await fetchJSON('/api/info', {retry: false});
      if (generation !== instantGeneration) return;
      instantURL = info.instant;
    }
    if (!instantURL) throw new Error('Common searches are not ready yet.');
    const data = await fetchJSON(instantURL, {retry: false});
    if (generation === instantGeneration) instant = data;
  } catch (error) {
    if (generation !== instantGeneration) return;
    if (error.status === 404) instantURL = null;
    if (retries.length) setTimeout(() => {
      if (generation === instantGeneration) loadInstant(retries.slice(1));
    }, retries[0]);
  }
}

revision.changed = () => {
  for (const cache of [searches, entries, characters]) cache.clear();
  instantGeneration++;
  instant = null;
  instantURL = null;
  loadInstant();
};

function instantAnswer(request) {
  const text = request.get('q');
  const answer = Object.hasOwn(instant?.searches ?? {}, text) && instant.searches[text];
  if (!answer || request.toString() !== new URLSearchParams({q: text, mode: 'auto', offset: 0, limit: SEARCH_PAGE_SIZE}).toString()) return null;
  // a key is the id, or "id~n" for a row showing another of its meanings
  const summaries = keys => keys.map(key => {
    const [simplified, traditional, pinyin, gloss, extra] = instant.entries[key];
    return {id: parseInt(key, 10), simplified, traditional: traditional || simplified, pinyin, gloss, ...extra};
  });
  const lists = Object.fromEntries(['results', 'pinyin', 'english'].filter(name => answer[name]).map(name => [name, summaries(answer[name])]));
  return {...answer, ...lists};
}

export {
  preferences, HAN, escapeHTML, state, SOURCE_NAMES, LONG_HEADWORD, CHARACTER_PREFETCH, characters,
  $, content, mobileView, setDetailOpen, icon, restartAnimation, $$, reducedMotion, motion,
  abortPending, CHARACTER_SENSES, retryFocus, begin, beginLoading, current, toast, popup, present,
  LOOKUP_MAX_HEIGHT, LOOKUP_RESULT_LIMIT, fetchJSON, entries, hold, SHEET_FLICK,
  SHEET_CLOSE_DISTANCE, entryState, listStateHTML, shown, query, searchMode, SEARCH_PAGE_SIZE,
  searches, instantAnswer, revision, PREFETCH_REQUEST_LIMIT, retryBusy, isHome, writeStored,
  openDisclosures, holdTransitions, applyTheme, TABS, stateHTML, SEARCH_MODES,
  mobileQuery, syncMobilePanes, loadInstant, abortAll,
};
