import {initializeHandwriting, canAddCharacter} from "/handwriting.js";
import {
  query, isHome, $, reducedMotion, motion, state, searchMode, openDisclosures, mobileView,
  setDetailOpen, entryState, revision, current, begin, content, entries, TABS, abortPending,
  beginLoading, $$, retryBusy, stateHTML, escapeHTML, retryFocus, toast, preferences, popup,
  SEARCH_MODES, mobileQuery, syncMobilePanes, loadInstant, abortAll,
} from "/shared.js";
import {initializeEntry} from "/entry.js";
import {initializeSearch} from "/search.js";
import {initializeSettings, renderSettings} from "/settings.js";
import {initializeSuggestions, hideSuggest} from "/suggestions.js";
import {headword, readingText} from "/text.js";

// the build this page's code came from
const pageRevision = revision.current;

function initializeDictionary() {
  const handwriting = initializeHandwriting(character => {
    if (!canAddCharacter(query, character)) return false;
    query.setRangeText(character, query.selectionStart, query.selectionEnd, 'end');
    setSearch(query.value);
    showResultsForTyping();
    search();
  });
  const {
    prefetchCharacters, summaryOf, showEntry, selectTab, renderEntry, renderTab, closeLookup,
    positionLookup, showLookup, renderPeek, moreWords, animateStrokes, retryCharacters, stopPlayback, speak,
  } = initializeEntry({go, updateTitle});
  const {loadResults, renderResults, markSelectedRow, noResultsState, moveSelection} = initializeSearch({
    search, searchAsTyped, setSearch, openFromHome, showResultsForTyping, closeLookup, updateTitle, selectEntry,
  });
  const {panels, isOpen, anchorPanels, closePanels, changeSetting} = initializeSettings({
    search, closeLookup, handwriting, renderResults, renderEntry, renderTab,
  });
  initializeSuggestions({openFromHome});

  function setHome(home, animate = false) {
    if (isHome() === home) return;
    const form = $('#search-form');
    const from = animate && !reducedMotion() ? form.getBoundingClientRect().top : null;
    hideSuggest();
    document.body.classList.toggle('home', home);
    document.body.classList.toggle('dictionary', !home);
    anchorPanels();
    const shift = from === null ? 0 : from - form.getBoundingClientRect().top;
    if (!shift) return;
    const animation = motion(form, [{translate: `0 ${shift}px`}, {translate: '0 0'}]);
    animation.finished.then(() => animation.cancel(), () => {});
  }

  function setSearch(text, scope = null) {
    if (query.value !== text) query.value = text;
    state.scope = scope === state.preference ? null : scope;
    renderSettings();
  }

  let pendingHistory = null;
  let historyTimer = null;
  let historyWritten = -Infinity;
  function flushHistory() {
    clearTimeout(historyTimer);
    historyTimer = null;
    if (!pendingHistory) return;
    history.replaceState(pendingHistory.view, '', pendingHistory.url);
    pendingHistory = null;
    historyWritten = performance.now();
  }
  function cancelHistory() {
    clearTimeout(historyTimer);
    historyTimer = null;
    pendingHistory = null;
  }
  addEventListener('pagehide', flushHistory);

  function go(intent, entry, {open = false, href = null} = {}) {
    if (intent === 'back') {
      flushHistory();
      return history.back();
    }
    const currentURL = pendingHistory?.url || location.href;
    const currentView = pendingHistory?.view || history.state;
    const here = new URL(currentURL).searchParams;
    const url = new URL(href || currentURL);
    if (entry !== undefined && !href) {
      if (!here.has('q')) intent = 'push';
      url.searchParams.set('q', state.query);
      for (const key of ['as_typed', 'handwriting', 'dictionary', 'word', 'reading', 'view']) url.searchParams.delete(key);
      if (searchMode() === 'auto') url.searchParams.delete('mode');
      else url.searchParams.set('mode', searchMode());
      if (entry) {
        url.searchParams.set('word', entry.simplified);
        url.searchParams.set('reading', entry.pinyin);
        if (open) url.searchParams.set('view', 'entry');
      }
    }
    // The new address is not drawn yet; do not save the current view under it.
    const view = intent === 'replace' && entry !== undefined && !href
      ? {...currentView, scroll: listScroll, tab: undefined, pane: undefined, disclosures: undefined}
      : {...currentView, ...viewState()};
    // After a confirmed new build this page's code is old: load the next step fresh, but not mid-typing
    const fresh = url.href !== currentURL && revision.current !== pageRevision && document.activeElement !== query;
    if (fresh && intent !== 'push') {
      abortAll();
      return location.replace(url);
    }
    if (intent === 'push' && url.href !== currentURL) {
      cancelHistory();
      history.replaceState(view, '', currentURL);
      // the step being left keeps its scroll and panels for Back
      if (fresh) {
        abortAll();
        return location.assign(url);
      }
      // Results can go back because the preceding step is this query's list.
      history.pushState({fromList: open && here.has('q') && here.get('view') !== 'entry'}, '', url);
      historyWritten = performance.now();
    } else {
      // Coalesce rapid arrow selections below Safari's history-write limit.
      pendingHistory = {view, url: url.href};
      if (performance.now() - historyWritten >= 150) flushHistory();
      else historyTimer ??= setTimeout(flushHistory, 150 - (performance.now() - historyWritten));
    }
    updateTitle();
  }

  // tracked on scroll so saving history never forces layout
  let listScroll = 0;
  let entryScroll = 0;
  $('.results-pane').addEventListener('scroll', event => { listScroll = event.currentTarget.scrollTop; }, {passive: true});
  const viewState = () => ({scroll: listScroll, tab: state.tab, pane: entryScroll, disclosures: openDisclosures()});

  function updateTitle() {
    const params = new URL(pendingHistory?.url || location.href).searchParams;
    const entry = state.detail?.entry;
    if (entry && params.get('word') === entry.simplified && params.get('reading') === entry.pinyin) {
      document.title = `${headword(entry)} ${readingText(entry)} · avkean 词典`;
    } else {
      document.title = state.query ? `${state.query} · avkean 词典` : 'avkean 词典 · Chinese–English dictionary';
    }
  }

  function searchAsTyped() {
    if (state.composing) return;
    search({push: !state.editingSearch});
    state.editingSearch = true;
  }

  async function showSearchSelection(data, {openMobile = false, entryId = null, saved = {}} = {}) {
    if (data.results.length || entryId !== null) {
      if (!mobileView() || openMobile || entryId !== null) {
        await selectEntry(entryId ?? data.results[0].id, {openMobile, intent: null, saved});
      } else {
        state.selected = null;
        state.detail = null;
        markSelectedRow();
      }
    } else {
      state.selected = null;
      state.detail = null;
      if (mobileView()) setDetailOpen(false);
      entryState(...noResultsState());
      updateTitle();
    }
  }

  // ownStep: on phones the word the Search key opens gets its own history step
  async function search({append = false, openMobile = false, entryId = null, push = false, ownStep = false} = {}) {
    if (state.composing) return;
    // a list from an older dictionary can't be continued
    if (append && state.resultsRevision !== revision.current) append = false;
    if (!append) {
      if (mobileView() && !openMobile && entryId === null) setDetailOpen(false);
      state.query = query.value.trim();
      go(push ? 'push' : 'replace', null);
    }
    const result = await loadResults(new URLSearchParams(), append);
    if (!result || append) return;
    await showSearchSelection(result.data, {openMobile, entryId});
    if (!current('search', result.controller) || !state.detail) return;
    if (entryId !== null) go('replace', state.detail.entry, {open: openMobile});
    else if (ownStep && mobileView()) go('push', state.detail.entry, {open: true});
  }

  async function selectEntry(id, {openMobile = true, intent = 'push', saved = {}} = {}) {
    const {tab = null} = saved;
    id = Number(id);
    if (intent) state.editingSearch = false;
    const controller = begin('entry');
    content.setAttribute('aria-busy', 'false');
    const known = summaryOf(id);
    const requestedRevision = state.resultsRevision || revision.current;
    if (intent && known) go(intent, known, {open: openMobile});
    if (state.detail?.entry.id === id && entries.peek(id) === state.detail) {
      state.selected = id;
      markSelectedRow();
      showEntry(openMobile);
      if (TABS.includes(tab) && tab !== state.tab) selectTab(tab);
      updateTitle();
      return;
    }
    const readingFocused = document.activeElement?.hasAttribute?.('data-reading');
    abortPending('character', 'related');
    stopPlayback();
    state.selected = id;
    state.summary = known;
    const previous = state.detail;
    state.detail = null;
    closeLookup();
    markSelectedRow();
    const finish = beginLoading(content, 'Loading word details.', controller.signal);
    // on phones the slide starts before the word is drawn
    const sliding = openMobile && mobileView() && !document.body.classList.contains('detail-open');
    showEntry(openMobile);
    try {
      const data = entries.get(id);
      const detail = data instanceof Promise ? await data : data;
      if (sliding) await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      if (!current('entry', controller)) return;
      // A rebuilt dictionary may reuse this ID for another word.
      if (known && (known.simplified !== detail.entry.simplified || known.pinyin !== detail.entry.pinyin)) return location.reload();
      const sameWord = previous?.entry.simplified === detail.entry.simplified;
      state.detail = detail;
      // skip if the page was closed while loading
      if (intent && !known && (!openMobile || document.body.classList.contains('detail-open'))) go(intent, detail.entry, {open: openMobile});
      if (TABS.includes(tab)) state.tab = tab;
      else if (!sameWord || !TABS.includes(state.tab)) state.tab = 'meanings';
      finish('Word details ready.');
      renderEntry({animate: true});
      prefetchCharacters(detail);
      $$('details', content).forEach((details, index) => { details.open = saved.disclosures?.[index] ?? details.open; });
      entryScroll = saved.pane ?? 0;
      $('#entry-pane').scrollTo({top: entryScroll, behavior: 'instant'});
      go('replace');
      if (readingFocused) ($('[data-reading]') || $('.entry-word'))?.focus({preventScroll: true});
      else if (openMobile && mobileView()) $('.entry-word')?.focus({preventScroll: true});
    } catch (error) {
      if (!current('entry', controller) || error.name === 'AbortError') return;
      if (error.status === 404 && revision.current !== requestedRevision) return location.reload();
      state.detail = null;
      const retried = retryBusy('entry');
      content.setAttribute('aria-busy', 'false');
      content.innerHTML = stateHTML('h1', '字', 'Word unavailable', escapeHTML(error.message), '<button type="button" class="text-link" data-retry-entry>Try again</button>');
      finish('Word details unavailable.');
      if (retried || (openMobile && mobileView())) retryFocus('entry');
    } finally {
      finish();
    }
  }

  function showResultsForTyping() {
    setHome(false, true);
    if (mobileView()) setDetailOpen(false);
  }

  function backToResults() {
    setDetailOpen(false);
    go(history.state?.fromList ? 'back' : 'push', null);
    $('.result-row.selected')?.focus({preventScroll: true});
  }

  function lookup(word, entryId = null) {
    closeLookup();
    setSearch(word, entryId !== null ? 'chinese' : null);
    closePanels();
    search({openMobile: true, entryId, push: true});
  }

  function searchText(text, scope = null) {
    closeLookup();
    setSearch(text, scope);
    closePanels();
    showResultsForTyping();
    search({push: true});
  }

  function openFromHome(entryId = null) {
    hideSuggest();
    if (entryId !== null && mobileView()) query.blur();
    showResultsForTyping();
    search({openMobile: entryId !== null, entryId, push: true});
    state.editingSearch = false;
  }

  document.addEventListener('click', event => {
    const button = event.target.closest('button');
    if (!event.target.closest('#lookup-popover') && !button?.dataset.lookup) closeLookup();
    if (!button) return;
    const data = button.dataset;
    if (data.entry !== undefined || data.reading !== undefined) selectEntry(Number(data.entry ?? data.reading));
    else if (data.lookup) showLookup(data.lookup, button, event.detail === 0);
    else if (data.peekEntry !== undefined) {
      state.peek.keyboard = event.detail === 0;
      renderPeek(Number(data.peekEntry)).catch(error => error.name !== 'AbortError' && toast(error.message));
    } else if (data.searchText !== undefined) searchText(data.searchText, data.searchScope);
    else if (data.openLookup) lookup(data.openLookup, data.openEntry !== undefined ? Number(data.openEntry) : null);
    else if (data.tab) selectTab(data.tab);
    else if (button.hasAttribute('data-more-words')) moreWords(button);
    else if (data.play) animateStrokes(button);
    if (button.hasAttribute('data-close')) closePanels(true);
    if (button.hasAttribute('data-close-lookup')) closeLookup(true);
    if (button.hasAttribute('data-back')) backToResults();
    if (button.hasAttribute('data-speak')) speak();
    if (button.hasAttribute('data-toggle-script')) changeSetting('script', preferences.traditional ? 'simplified' : 'traditional');
    const retry = ['entry', 'characters', 'search'].find(kind => button.hasAttribute(`data-retry-${kind}`));
    if (retry && retryBusy(retry)) return;
    if (retry) for (const item of $$(`[data-retry-${retry}]`)) item.setAttribute('aria-busy', 'true');
    if (retry === 'entry') selectEntry(state.selected);
    if (retry === 'characters') retryCharacters();
    if (retry === 'search') search();
    if (button.hasAttribute('data-search-all')) changeSetting('mode', 'auto');
  });

  for (const type of ['pointerover', 'focusin']) {
    document.addEventListener(type, event => {
      const row = event.target.closest('[data-entry], [data-reading], [data-peek-entry]');
      if (row && !row.closest('[inert]')) entries.prefetch(Number(row.dataset.entry ?? row.dataset.reading ?? row.dataset.peekEntry));
      if (event.target.closest('[data-tab="characters"]')) prefetchCharacters();
    });
  }

  document.addEventListener('keydown', event => {
    if (event.defaultPrevented || event.isComposing || state.composing || event.keyCode === 229) return;
    const editing = event.target.matches('input, textarea, select, [contenteditable]');
    if (event.key === '/' && !editing) {
      event.preventDefault();
      query.focus();
      query.select();
      return;
    }
    if (event.key === 'Escape') {
      if (event.target === query) event.preventDefault();
      if (!popup.hidden) return closeLookup(true);
      if (panels.some(isOpen)) return closePanels(true);
      if (document.body.classList.contains('detail-open')) backToResults();
      return;
    }
    const sentence = event.target.closest('[data-lookup-words]');
    if (sentence && ['ArrowLeft', 'ArrowRight'].includes(event.key)) {
      event.preventDefault();
      const words = $$('.word-link', sentence);
      const next = words[words.indexOf(event.target) + (event.key === 'ArrowRight' ? 1 : -1)];
      if (!next) return;
      event.target.tabIndex = -1;
      next.tabIndex = 0;
      next.focus();
      return;
    }
    if (event.target.closest('[role="tablist"]') && ['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
      event.preventDefault();
      const tabs = $$('[data-tab]');
      const index = tabs.indexOf(event.target);
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : (index + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length;
      tabs[next].focus();
      selectTab(tabs[next].dataset.tab);
      return;
    }
    if (!isHome() && event.target === query && event.key === 'ArrowDown' && $('#results .result-row')) {
      event.preventDefault();
      moveSelection(0);
    } else if (!editing && event.target.closest('.results-pane') && ['ArrowDown', 'ArrowUp'].includes(event.key)) {
      event.preventDefault();
      moveSelection(event.key === 'ArrowDown' ? 1 : -1);
    }
  });

  async function restoreLocation(event) {
    cancelHistory();
    hideSuggest();
    state.editingSearch = false;
    const params = new URLSearchParams(location.search);
    const saved = history.state || {};
    const text = (params.get('q') || '').trim();
    const mode = SEARCH_MODES.includes(params.get('mode')) ? params.get('mode') : 'auto';
    const previousMode = searchMode();
    // Address auto/fulltext persists as a preference; chinese/english lasts one search.
    if (['auto', 'fulltext'].includes(mode)) state.preference = mode;
    setSearch(params.get('q') || '', mode);
    const open = params.get('view') === 'entry';
    if (event && text && text === state.query && searchMode() === previousMode && state.results.length && !$('#results').inert) {
      const word = params.get('word');
      const reading = params.get('reading');
      const match = item => item.simplified === word && item.pinyin === reading;
      const id = word
        ? (state.results.find(match) ?? (state.detail?.entry.simplified === word ? state.detail.readings?.find(item => item.pinyin === reading) : null))?.id
        : mobileView() ? null : state.results[0].id;
      if (!word || id !== undefined) {
        closeLookup();
        if (id === null) {
          setDetailOpen(false);
          $('.result-row.selected')?.focus({preventScroll: true});
          updateTitle();
        } else {
          await selectEntry(id, {openMobile: open, intent: null, saved});
        }
        return;
      }
    }
    setHome(!params.has('q'));
    // an open word page stays open while the search loads
    if (!open) setDetailOpen(false);
    if (!state.composing) {
      state.query = text;
      updateTitle();
    }
    const result = await loadResults(params);
    if (!result) return;
    if (params.has('word') && result.data.selected === undefined) go('replace', null);
    if (saved.scroll) $('.results-pane').scrollTop = listScroll = saved.scroll;
    await showSearchSelection(result.data, {entryId: result.data.selected ?? null, openMobile: open, saved});
  }

  window.addEventListener('popstate', restoreLocation);
  document.addEventListener('click', event => {
    const link = event.target.closest('a[href="/"], a[href^="/?"]');
    if (!link || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    closePanels();
    go('push', undefined, {href: link.href});
    restoreLocation();
    if (isHome()) query.focus({preventScroll: true});
  });
  mobileQuery.addEventListener('change', () => {
    syncMobilePanes();
    if (!mobileView() && !state.detail && state.results.length) selectEntry(state.results[0].id, {openMobile: false});
  });
  window.addEventListener('resize', () => {
    positionLookup();
    anchorPanels();
  });
  $('#entry-pane').addEventListener('scroll', event => {
    entryScroll = event.currentTarget.scrollTop;
    if (state.peek.keyboard && (popup.contains(document.activeElement) || state.peek.returnTo === document.activeElement)) positionLookup();
    else closeLookup();
  }, {passive: true});

  // a word link opened directly shows its page at once; only changes get the slide
  if (mobileView() && new URLSearchParams(location.search).get('view') === 'entry') {
    document.body.classList.add('no-slide');
    setDetailOpen(true);
    void document.body.offsetWidth;
    document.body.classList.remove('no-slide');
  }
  syncMobilePanes();
  renderSettings();
  restoreLocation().finally(() => (window.requestIdleCallback || (callback => setTimeout(callback, 1000)))(() => loadInstant()));
  if (isHome()) query.focus({preventScroll: true});
}

initializeDictionary();
