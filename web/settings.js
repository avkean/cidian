import {
  shown, preferences, $$, $, present, mobileView, isHome, reducedMotion, searchMode, state,
  writeStored, openDisclosures, content, holdTransitions, applyTheme, restartAnimation, query,
} from "/shared.js";
import {hideSuggest} from "/suggestions.js";
import {scriptFormats, entryCharacters} from "/text.js";


const SCOPE_HINTS = {auto: 'This search looks up words, not full text.', chinese: 'This search looks only at Chinese.', english: 'This search looks only at English.'};

function settingValue(name) {
  if (name === 'mode') return shown().scope ? null : shown().preference;
  if (name === 'script') return preferences.traditional ? 'traditional' : 'simplified';
  if (name === 'examplePinyin') return preferences.examplePinyin ? 'on' : 'off';
  return preferences[name];
}

let renderedSettings = null;

function renderSettings() {
  const key = [shown().scope, shown().preference, preferences.traditional, preferences.pinyin, preferences.theme, preferences.examplePinyin].join();
  if (key === renderedSettings) return;
  renderedSettings = key;
  for (const group of $$('[data-setting]')) {
    const value = settingValue(group.dataset.setting);
    const buttons = $$('button', group);
    for (const button of buttons) {
      const checked = button.dataset.value === value;
      button.setAttribute('aria-checked', String(checked));
      button.tabIndex = checked || (value === null && button === buttons[0]) ? 0 : -1;
    }
  }
  $('#mode-hint').textContent = SCOPE_HINTS[shown().scope] || $(`[data-setting="mode"] [data-value="${shown().preference}"]`).dataset.hint;
}
function initializeSettings({search, closeLookup, handwriting, renderResults, renderEntry, renderTab}) {

  const panels = ['handwriting', 'settings'].map(name => ({name, element: $(`#${name}-panel`), toggle: $(`#${name}-toggle`)}));
  const isOpen = panel => !panel.element.hidden;

  function setPanel(panel, open) {
    present(panel.element, open);
    if (open) {
      panel.element.scrollTop = 0;
      if (panel.name === 'handwriting') hideSuggest();
      anchorPanel(panel);
    }
    panel.toggle.setAttribute('aria-expanded', String(open));
    if (panel.name === 'settings') present($('.backdrop'), open);
  }

  function anchorPanel(panel) {
    const element = panel.element;
    element.style.removeProperty('right');
    element.style.removeProperty('transform-origin');
    if (mobileView() || isHome()) return;
    const bar = panel.toggle.closest('.search-form, .header-tools').getBoundingClientRect();
    const layer = element.offsetParent.getBoundingClientRect();
    const width = element.offsetWidth;
    const right = Math.max(8, Math.min(layer.right - bar.right, layer.width - width - 8));
    const toggle = panel.toggle.getBoundingClientRect();
    element.style.right = `${right}px`;
    element.style.transformOrigin = `${toggle.left + toggle.width / 2 - (layer.right - right - width)}px top`;
  }

  const anchorPanels = () => panels.filter(isOpen).forEach(anchorPanel);

  function closePanels(restoreFocus = false) {
    let opener = null;
    for (const panel of panels) {
      if (isOpen(panel)) opener = panel.toggle;
      setPanel(panel, false);
    }
    if (restoreFocus) opener?.focus({preventScroll: true});
  }

  for (const panel of panels) panel.element.addEventListener('focusout', event => {
    const next = event.relatedTarget;
    const editing = panel.name === 'handwriting' && $('#search-form').contains(next);
    if (next && !event.currentTarget.contains(next) && next !== panel.toggle && !editing) closePanels();
  });

  function togglePanel(name) {
    const panel = panels.find(panel => panel.name === name);
    const opening = !isOpen(panel);
    closePanels();
    if (!opening) return;
    setPanel(panel, true);
    if (name === 'handwriting') {
      handwriting.redraw();
      // The drawn box is shorter mid-motion; scroll to its full size.
      if (isHome()) {
        const below = panel.element.offsetParent.getBoundingClientRect().top + panel.element.offsetTop + panel.element.offsetHeight - innerHeight;
        if (below > 0) scrollBy({top: below, behavior: reducedMotion() ? 'instant' : 'smooth'});
      }
      $('#handwriting-canvas').focus({preventScroll: true});
    } else {
      $('[data-setting] [tabindex="0"]', panel.element)?.focus({preventScroll: true});
    }
  }

  function changeSetting(name, value) {
    if (value === settingValue(name)) return;
    if (name === 'mode') {
      const before = searchMode();
      const text = query.value;
      if (value === 'auto') query.value = text.replace(/^[\s#]+/, '').trim();
      state.preference = value;
      state.scope = null;
      renderSettings();
      if (searchMode() !== before || query.value !== text) search();
      return;
    }
    if (name === 'script') preferences.traditional = value === 'traditional';
    if (name === 'pinyin') preferences.pinyin = value;
    if (name === 'theme') preferences.theme = value;
    if (name === 'examplePinyin') preferences.examplePinyin = value === 'on';
    writeStored('cidian.preferences', preferences);
    renderSettings();
    closeLookup();
    if (name === 'theme') {
      switchTheme();
    } else if (name === 'script') {
      applyScript();
    } else {
      redraw();
    }
  }

  // Pinyin lines come and go, so exclude them from scroll anchors.
  function redraw() {
    const pane = $('#entry-pane');
    const open = openDisclosures();
    const lines = () => [...$$('.example-chinese, .example-english', content), ...$$('.sense', content)];
    const top = pane.getBoundingClientRect().top;
    const line = lines().findIndex(item => item.getBoundingClientRect().height && item.getBoundingClientRect().top >= top);
    const offset = lines()[line]?.getBoundingClientRect().top;
    renderResults();
    renderEntry({animate: false});
    $$('details', content).forEach((details, index) => { details.open = open[index] ?? details.open; });
    if (line >= 0) pane.scrollTop += lines()[line].getBoundingClientRect().top - offset;
  }

  // Crossfade the page to keep its colours in sync.
  function switchTheme() {
    const swap = () => {
      holdTransitions();
      applyTheme();
      handwriting.redraw();
    };
    if (document.startViewTransition && !reducedMotion()) {
      // A newer setting can replace the transition before its snapshot is ready.
      document.startViewTransition(swap).ready.catch(() => {});
    }
    else swap();
  }

  function applyScript() {
    for (const element of $$('[data-simplified]')) {
      const text = preferences.traditional ? element.dataset.traditional : element.dataset.simplified;
      if (element.dataset.scriptFormat) {
        element.innerHTML = scriptFormats[element.dataset.scriptFormat](text);
      } else if (element.textContent !== text) element.textContent = text;
    }
    for (const element of $$('[data-script-lookup]')) element.dataset.lookup = element.textContent;
    const toggle = $('[data-toggle-script]');
    if (toggle) {
      const label = `Show ${preferences.traditional ? 'simplified' : 'traditional'} characters`;
      toggle.title = label;
      toggle.setAttribute('aria-label', `${toggle.textContent}: ${label}`);
    }
    const wordline = $('.entry-wordline');
    if (wordline) restartAnimation(wordline, 'script-swap');
    const count = $('#character-count');
    if (count) {
      const length = entryCharacters().length;
      count.textContent = length > 1 ? length : '';
      count.hidden = length < 2;
    }
    if (state.tab === 'characters') renderTab({animate: false});
  }
  for (const panel of panels) panel.toggle.addEventListener('click', () => togglePanel(panel.name));
  for (const group of $$('[data-setting]')) {
    group.addEventListener('click', event => {
      const button = event.target.closest('button[data-value]');
      if (button) changeSetting(group.dataset.setting, button.dataset.value);
    });
    group.addEventListener('keydown', event => {
      if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) return;
      event.preventDefault();
      const buttons = $$('button', group);
      const index = buttons.indexOf(document.activeElement);
      const next = buttons[(index + (['ArrowRight', 'ArrowDown'].includes(event.key) ? 1 : -1) + buttons.length) % buttons.length];
      changeSetting(group.dataset.setting, next.dataset.value);
      $(`button[data-value="${next.dataset.value}"]`, group).focus();
    });
  }

  // Let the backdrop consume the whole tap; WebKit requires its click listener.
  document.addEventListener('pointerdown', event => {
    const target = event.target;
    if (target.closest('.backdrop')) return;
    if (panels.some(panel => isOpen(panel) && !panel.element.contains(target) && !panel.toggle.contains(target))) closePanels();
  });
  $('.backdrop').addEventListener('click', () => closePanels(true));
  return {panels, isOpen, anchorPanels, closePanels, changeSetting};
}

export {renderSettings, initializeSettings};
