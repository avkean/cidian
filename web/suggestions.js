import {
  $, $$, isHome, abortPending, searches, query, state, begin, searchMode, SEARCH_PAGE_SIZE,
  instantAnswer, current, escapeHTML,
} from "/shared.js";
import {rowContentHTML} from "/text.js";

const suggest = $('#suggest');
let suggestTimer = 0;
let suggestKey = null;
let suggestActive = -1;
let suggestText = '';
const suggestItems = () => $$('[role="option"]', suggest);
const homeSuggests = () => isHome() && $('#handwriting-panel').hidden;

function cancelSuggest() {
  suggest.classList.remove('opening');
  clearTimeout(suggestTimer);
  abortPending('suggest');
  if (suggestKey !== null) searches.abort(suggestKey);
  suggestKey = null;
}

function hideSuggest() {
  cancelSuggest();
  suggest.hidden = true;
  suggest.replaceChildren();
  suggestActive = -1;
  suggestText = '';
  query.setAttribute('aria-expanded', 'false');
  query.removeAttribute('aria-activedescendant');
}

function markSuggest(index) {
  const items = suggestItems();
  const next = Math.max(-1, Math.min(items.length - 1, index));
  if (next === suggestActive) return;
  suggestActive = next;
  items.forEach((item, i) => {
    item.classList.toggle('active', i === next);
    item.setAttribute('aria-selected', String(i === next));
  });
  if (next < 0) query.removeAttribute('aria-activedescendant');
  else query.setAttribute('aria-activedescendant', items[next].id);
}

async function fetchSuggest() {
  if (!homeSuggests() || state.composing) return;
  const text = query.value.trim();
  if (!text) return hideSuggest();
  const controller = begin('suggest');
  const request = new URLSearchParams({q: text, mode: searchMode(), offset: 0, limit: SEARCH_PAGE_SIZE});
  suggestKey = request.toString();
  try {
    const answer = instantAnswer(request) || searches.get(suggestKey);
    const data = answer instanceof Promise ? await answer : answer;
    if (!current('suggest', controller) || !homeSuggests() || text !== query.value.trim()) return;
    const opening = suggest.hidden;
    suggest.innerHTML = data.results.slice(0, 5).map((entry, i) => `<div role="option" id="suggest-${i}" aria-selected="false" class="result-row" data-suggest-entry="${entry.id}">${rowContentHTML(entry)}</div>`).join('')
      + `<div role="option" id="suggest-more" aria-selected="false" class="more">All results for “${escapeHTML(text)}”<span aria-hidden="true">→</span></div>`;
    suggest.classList.toggle('opening', opening);
    suggest.hidden = false;
    suggestActive = -1;
    suggestText = text;
    query.removeAttribute('aria-activedescendant');
    query.setAttribute('aria-expanded', 'true');
    const count = data.results.slice(0, 5).length;
    $('#result-status').textContent = `${count} ${count === 1 ? 'suggestion' : 'suggestions'}`;
  } catch (error) {
    if (current('suggest', controller) && error.name !== 'AbortError') hideSuggest();
  }
}

function suggestAsTyped() {
  cancelSuggest();
  markSuggest(-1);
  if (!query.value.trim()) return hideSuggest();
  if (!state.composing) suggestTimer = setTimeout(fetchSuggest, 120);
}
function initializeSuggestions({openFromHome}) {

  suggest.addEventListener('mousedown', event => event.preventDefault());
  suggest.addEventListener('pointermove', event => {
    const item = event.target.closest('[role="option"]');
    if (item) markSuggest(suggestItems().indexOf(item));
  });
  suggest.addEventListener('pointerleave', () => markSuggest(-1));
  suggest.addEventListener('click', event => {
    const row = event.target.closest('[data-suggest-entry]');
    if (row) openFromHome(Number(row.dataset.suggestEntry));
    else if (event.target.closest('.more')) openFromHome();
  });
  query.addEventListener('keydown', event => {
    if (event.isComposing || state.composing || event.keyCode === 229 || !homeSuggests()) return;
    if (event.key === 'Escape') {
      const showing = !suggest.hidden;
      hideSuggest();
      if (showing) event.preventDefault();
      return;
    }
    if (suggest.hidden) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      markSuggest(suggestActive + (event.key === 'ArrowDown' ? 1 : -1));
    } else if (event.key === 'Enter' && suggestActive >= 0 && suggestText === query.value.trim()) {
      event.preventDefault();
      suggestItems()[suggestActive].click();
    }
  });
}

export {homeSuggests, hideSuggest, cancelSuggest, suggestAsTyped, initializeSuggestions};
