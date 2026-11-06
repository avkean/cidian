import {
  $, beginLoading, entryState, listStateHTML, state, shown, escapeHTML, HAN, query, abortPending,
  content, begin, searchMode, SEARCH_PAGE_SIZE, searches, instantAnswer, current, revision,
  PREFETCH_REQUEST_LIMIT, entries, toast, retryBusy, mobileView, setDetailOpen, retryFocus, icon, $$,
  reducedMotion, isHome,
} from "/shared.js";
import {renderSettings} from "/settings.js";
import {rowContentHTML, scriptData, headword, displayText} from "/text.js";
import {homeSuggests, hideSuggest, cancelSuggest, suggestAsTyped} from "/suggestions.js";

function initializeSearch({
  search, searchAsTyped, setSearch, openFromHome, showResultsForTyping, closeLookup, updateTitle, selectEntry,
}) {
  let finishSearch = null;
  let homeMessage = false;

  function setSearchBusy(busy) {
    const list = $('#results');
    list.inert = busy;
    $('#load-more').setAttribute('aria-disabled', String(busy));
    if (busy) {
      finishSearch ||= beginLoading(list, 'Searching.');
    } else {
      finishSearch?.();
      finishSearch = null;
    }
  }

  function showState(...message) {
    entryState(...message);
    $('#results').innerHTML = listStateHTML(...message);
    renderedList = null;
    $('#load-more').hidden = true;
  }

  function noResultsState() {
    if (state.truncated) return ['词', 'Pattern too broad', 'Add more characters or pinyin to narrow the pattern.'];
    const words = state.query.split(/\s+/);
    const keyWord = words.reduce((longest, word) => word.length > longest.length ? word : longest, '');
    const mode = shown().scope || shown().preference;
    const title = `No results for “${escapeHTML(state.query)}”`;
    const narrowed = {chinese: 'Only Chinese is being searched.', english: 'Only English is being searched.', fulltext: 'Full text searches headwords, meanings and examples.'}[mode];
    if (narrowed) return ['词', title, narrowed, `<button type="button" class="text-link" data-search-all>${mode === 'fulltext' ? 'Look up as words' : 'Search everything'}</button>`];
    const advice = HAN.test(state.query)
      ? 'Check the characters, try fewer of them, or search in pinyin or English instead.'
      : words.length > 1
        ? `Check the spelling, or search for one key word, such as <a href="/?${escapeHTML(new URLSearchParams({q: keyWord}))}">${escapeHTML(keyWord)}</a>.`
        : 'Check the spelling, or search in Chinese characters or pinyin instead.';
    return ['词', title, advice];
  }

  function clearResults() {
    Object.assign(state, {served: null, results: [], segments: [], pinyin: [], english: [], capitals: '', related: '', unfound: [], total: 0, truncated: false, selected: null, detail: null});
    renderSettings();
  }

  async function loadResults(params, append = false) {
    if (state.composing) return;
    if (append && query.value.trim() !== state.query) return;
    const text = query.value.trim();
    state.query = text;
    syncClear();
    closeLookup();
    if (!text) {
      setSearchBusy(false);
      abortPending('search', 'entry', 'character');
      clearResults();
      renderResults();
      $('#result-status').textContent = 'Ready';
      showState('字', 'Look up a word', 'Type Chinese characters, pinyin or an English word.');
      homeMessage = true;
      updateTitle();
      return;
    }
    // Hide the empty-query message before the answer arrives.
    if (homeMessage) {
      homeMessage = false;
      $('#results').replaceChildren();
      content.replaceChildren();
    }
    const controller = begin('search');
    const more = $('#load-more');
    const pagingFocus = append && more === document.activeElement && more.matches(':focus-visible');
    const firstNew = state.results.length;
    if (!append) abortPending('entry', 'character');
    setSearchBusy(true);
    const request = new URLSearchParams({q: text, mode: searchMode(), offset: append ? state.results.length : 0, limit: SEARCH_PAGE_SIZE});
    for (const key of ['word', 'reading']) if (params.has(key)) request.set(key, params.get(key));
    let refocus = false;
    try {
      const key = request.toString();
      if (state.latestSearch !== key) searches.abort(state.latestSearch);
      state.latestSearch = key;
      // Render cached answers in the keystroke's frame.
      const answer = instantAnswer(request) || searches.get(key);
      const data = answer instanceof Promise ? await answer : answer;
      if (!current('search', controller)) return;
      if (append && state.resultsRevision !== revision.current) return search();
      if (!append) state.resultsRevision = revision.current;
      state.results = append ? [...state.results, ...data.results] : data.results;
      state.segments = data.segments || [];
      if (!append) {
        state.pinyin = data.pinyin || [];
        state.english = data.english || [];
        state.capitals = data.capitals || '';
        state.related = data.related || '';
        state.unfound = data.unfound || [];
      }
      state.total = data.total;
      state.truncated = Boolean(data.truncated);
      state.served = data.mode;
      renderSettings();
      const focusMore = pagingFocus && document.activeElement === more;
      renderResults({append});
      if (!append) $('.results-pane').scrollTop = 0;
      setSearchBusy(false);
      if (focusMore) {
        const row = $$('.result-row', $('#results'))[firstNew];
        row?.focus();
        markSelectedRow(row);
      }
      if (!append) for (const entry of state.results.slice(0, PREFETCH_REQUEST_LIMIT)) entries.prefetch(entry.id);
      return {data, controller};
    } catch (error) {
      if (!current('search', controller) || error.name === 'AbortError') return;
      if (append) {
        toast(error.message);
        return;
      }
      refocus = retryBusy('search');
      clearResults();
      $('#result-status').textContent = 'Search unavailable';
      if (mobileView()) setDetailOpen(false);
      showState('字', 'Search unavailable', escapeHTML(error.message), '<button type="button" class="text-link" data-retry-search>Try again</button>');
      updateTitle();
    } finally {
      if (current('search', controller)) setSearchBusy(false);
      // the list is inert until it stops being busy
      if (refocus) retryFocus('search');
    }
  }

  function resultRowHTML(entry) {
    const selected = state.selected === entry.id;
    return `<button type="button" class="result-row${selected ? ' selected' : ''}" data-entry="${entry.id}" tabindex="-1"${selected ? ' aria-current="true"' : ''}>${rowContentHTML(entry)}</button>`;
  }

  function otherReadingHTML(entries, label, text, mode) {
    if (!entries.length) return '';
    const words = entries.map(entry => `<button type="button" class="note-word" lang="zh" data-entry="${entry.id}" ${scriptData(entry)}>${escapeHTML(headword(entry))}</button>`).join(' ');
    return `<p class="phrase-note"><a class="text-link" href="/?${escapeHTML(new URLSearchParams({q: text, mode}))}">${label} ${escapeHTML(text)}</a>: ${words}</p>`;
  }

  let renderedList = '';

  function renderResults({append = false} = {}) {
    const list = $('#results');
    const focused = document.activeElement?.closest?.('.result-row')?.dataset.entry;
    const breakdown = state.segments.length > 0;
    const english = !state.segments.some(segment => HAN.test(segment.text));
    $('#result-status').textContent = breakdown ? (english ? 'No meaning has the whole phrase' : 'Words in this phrase') : state.truncated ? (state.total ? `Showing the best ${state.total.toLocaleString()}` : 'Pattern too broad') : `${state.total.toLocaleString()} ${state.total === 1 ? 'result' : 'results'}`;
    let html = '';
    if (breakdown) {
      if (english) html += `<p class="phrase-note" data-phrase-note>No meaning has the whole phrase.${state.unfound.length ? ` Nothing found for ${state.unfound.map(word => `\u201c${escapeHTML(word)}\u201d`).join(', ')}.` : ''}</p>`;
      const byId = new Map(state.results.map(entry => [entry.id, entry]));
      const seen = new Set();
      for (const segment of state.segments) {
        const text = segment.text.trim();
        const rows = segment.ids.filter(id => byId.has(id) && !seen.has(id));
        // Only unknown characters are errors; parts beyond the search limit stay links.
        const unsearched = !segment.ids.length && /[\p{L}\p{N}]/u.test(text);
        const missing = unsearched && HAN.test(text);
        if (!rows.length && !unsearched) continue;
        const label = `<span${HAN.test(text) ? ' lang="zh"' : ''}>${displayText(text, segment)}</span>`;
        html += missing
          ? `<p class="segment-label">${label}<span class="faint">No entry</span></p>`
          : `<button type="button" class="segment-label" data-search-text="${escapeHTML(text)}" aria-label="All results for ${escapeHTML(text)}">${label}${icon('chevron')}</button>`;
        for (const id of rows) {
          seen.add(id);
          html += resultRowHTML(byId.get(id));
        }
      }
    } else if (state.results.length) {
      const start = append ? $$('.result-row', list).length : 0;
      if (!append) {
        if (state.related) html += `<p class="phrase-note" data-related>Showing results for <a class="text-link" href="/?${escapeHTML(new URLSearchParams({q: state.related, mode: 'english'}))}">${escapeHTML(state.related)}</a></p>`;
        html += otherReadingHTML(state.english, 'English', state.capitals, 'english');
        html += otherReadingHTML(state.pinyin, 'Pinyin', state.query, 'chinese');
      }
      html += state.results.slice(start).map(resultRowHTML).join('');
    } else if (state.query) {
      html = listStateHTML(...noResultsState());
    }
    // New rows move into place; rows that stay keep still while typing.
    const shown = new Set($$('.result-row', list).map(row => row.dataset.entry));
    if (append && !breakdown) list.insertAdjacentHTML('beforeend', html);
    else if (html !== renderedList) list.innerHTML = html;
    if (!reducedMotion()) for (const row of $$('.result-row', list)) if (!shown.has(row.dataset.entry)) row.classList.add('arriving');
    renderedList = append && !breakdown ? null : html;
    $('#load-more').hidden = state.results.length >= state.total;
    markSelectedRow();
    if (focused !== undefined) $$('.result-row', list).find(row => row.dataset.entry === focused)?.focus({preventScroll: true});
  }

  function markSelectedRow(focusedRow) {
    const rows = $$('.result-row', $('#results'));
    const stop = rows.find(row => row === focusedRow) || rows.find(row => Number(row.dataset.entry) === state.selected) || rows[0];
    for (const row of rows) {
      const selected = Number(row.dataset.entry) === state.selected;
      row.classList.toggle('selected', selected);
      row.tabIndex = row === stop ? 0 : -1;
      if (selected) row.setAttribute('aria-current', 'true');
      else row.removeAttribute('aria-current');
    }
  }

  function moveSelection(step) {
    const rows = $$('.result-row', $('#results'));
    if (!rows.length) return;
    const focused = rows.indexOf(document.activeElement);
    const from = focused >= 0 ? focused : rows.findIndex(row => Number(row.dataset.entry) === state.selected);
    if (focused === 0 && step < 0) return query.focus();
    const row = rows[Math.max(0, Math.min(rows.length - 1, from + step))];
    selectEntry(Number(row.dataset.entry), {openMobile: false, intent: 'replace'});
    row.focus({preventScroll: true});
    row.scrollIntoView({block: 'nearest'});
  }

  $('#search-form').addEventListener('click', event => {
    if (!event.target.closest('button, input')) query.focus();
  });
  $('#search-form').addEventListener('submit', event => {
    event.preventDefault();
    if (state.composing || event.isComposing) return;
    if (isHome() && !query.value.trim()) return;
    if (homeSuggests()) return openFromHome();
    hideSuggest();
    showResultsForTyping();
    if (!state.composing) search({openMobile: mobileView(), push: true, ownStep: true});
    state.editingSearch = false;
  });
  query.addEventListener('compositionstart', () => {
    state.composing = true;
    cancelSuggest();
    setSearchBusy(false);
    abortPending('search', 'entry', 'character');
  });
  query.addEventListener('compositionend', () => {
    state.composing = false;
    if (homeSuggests()) return suggestAsTyped();
    searchAsTyped();
  });
  query.addEventListener('input', event => {
    setSearch(query.value);
    if (homeSuggests()) return suggestAsTyped();
    showResultsForTyping();
    if (!event.isComposing) searchAsTyped();
  });
  const narrowField = matchMedia('(max-width: 379px)');
  const fitPlaceholder = () => { query.placeholder = narrowField.matches ? 'Search' : 'Chinese, pinyin, English'; };
  narrowField.addEventListener('change', fitPlaceholder);
  fitPlaceholder();

  const clearButton = $('#clear-query');
  const syncClear = () => { clearButton.hidden = !query.value; };
  query.addEventListener('input', syncClear);
  clearButton.addEventListener('click', () => {
    query.value = '';
    query.focus();
    query.dispatchEvent(new Event('input', {bubbles: true}));
  });
  query.addEventListener('beforeinput', event => {
    if (!event.inputType.startsWith('insert')) return;
    const added = event.data ?? event.dataTransfer?.getData('text') ?? '';
    const selected = query.selectionEnd - query.selectionStart;
    const full = !selected && query.value.length >= query.maxLength;
    if (full || (added && query.value.length - selected + added.length > query.maxLength)) toast(`Searches are limited to ${query.maxLength} characters.`);
  });
  $('#load-more').addEventListener('click', event => {
    if (event.currentTarget.getAttribute('aria-disabled') !== 'true') search({append: true});
  });
  return {loadResults, renderResults, markSelectedRow, noResultsState, moveSelection};
}

export {initializeSearch};
