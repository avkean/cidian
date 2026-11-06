import {
  state, CHARACTER_PREFETCH, characters, LONG_HEADWORD, escapeHTML, $, content, mobileView,
  setDetailOpen, preferences, icon, restartAnimation, $$, reducedMotion, motion, abortPending,
  CHARACTER_SENSES, retryFocus, begin, beginLoading, current, toast, popup, present,
  LOOKUP_MAX_HEIGHT, LOOKUP_RESULT_LIMIT, fetchJSON, entries, hold, SHEET_FLICK,
  SHEET_CLOSE_DISTANCE, revision,
} from "/shared.js";
import {
  entryCharacters, headword, scriptData, clausesHTML, readingText, displayText, traditionalOf,
  sourceName, examplePinyinHTML, rowContentHTML, pinyinText,
} from "/text.js";

function initializeEntry({go, updateTitle}) {
  function prefetchCharacters(detail = state.detail) {
    for (const item of entryCharacters(detail).slice(0, CHARACTER_PREFETCH)) characters.prefetch(item.character);
  }

  function pendingHTML() {
    const known = state.summary;
    if (!known) return '';
    const word = headword(known);
    return `<header class="entry-header" data-pending><div class="entry-heading"><div class="entry-wordline"><h1 class="entry-word${[...word].length >= LONG_HEADWORD ? ' long' : ''}" lang="zh" tabindex="-1" ${scriptData(known)} data-script-format="clauses">${clausesHTML(word)}</h1></div><div class="entry-sound"><span class="entry-pinyin">${escapeHTML(readingText(known))}</span></div></div></header>`;
  }

  function summaryOf(id) {
    const detail = state.detail;
    const readings = (detail?.readings || []).map(reading => ({simplified: detail.entry.simplified, traditional: detail.entry.traditional, ...reading}));
    const characterEntries = [...characterResults.values()].flatMap(result => result.data?.entries || []);
    return [state.results, state.pinyin, state.english, detail?.related || [], readings, characterEntries].flat().find(item => item.id === id) || null;
  }

  // While loading, replace the phone heading so it never shows the previous word.
  function showEntry(openMobile) {
    if (state.detail?.entry.id !== state.selected) {
      if ($('[data-retry-entry]', content)) for (const child of content.children) child.inert = true;
      else if (mobileView() || !$('.entry-tabs', content)) content.innerHTML = pendingHTML();
      else for (const child of content.children) child.inert = true;
    }
    setDetailOpen(openMobile);
    if (openMobile && mobileView()) ($('.entry-word[tabindex]', content) || $('.app-header [data-back]')).focus({preventScroll: true});
  }

  function otherReadingsHTML(detail, attribute) {
    const others = (detail.readings || []).filter(reading => reading.id !== detail.entry.id);
    if (!others.length) return '';
    return `<span class="other-readings" role="group" aria-label="Other pronunciations">also ${others.map(reading => `<button type="button" class="reading-link" ${attribute}="${reading.id}" aria-label="Pronunciation ${escapeHTML(readingText(detail.entry, reading))}">${escapeHTML(readingText(detail.entry, reading))}</button>`).join('')}</span>`;
  }

  function usageHTML(entry, lone = false) {
    const notes = [];
    if (entry.hsk) notes.push(`<span title="HSK 3.0 level">HSK ${entry.hsk === 7 ? '7–9' : entry.hsk}</span>`);
    if (entry.classifiers?.length) {
      notes.push(`<span>Measure word${entry.classifiers.length > 1 ? 's' : ''} <span class="note-links">${entry.classifiers.slice(0, 4).map(word => `<button type="button" class="word-link" data-lookup="${escapeHTML(preferences.traditional ? entry.traditional_text?.[word] || word : word)}" lang="zh" data-script-lookup>${displayText(word, entry)}</button>`).join(' ')}</span></span>`);
    }
    return notes.length ? `<p class="usage-notes${lone ? ' lone' : ''}">${notes.join('')}</p>` : '';
  }

  function renderEntry({animate = false} = {}) {
    const detail = state.detail;
    if (!detail) {
      if ($('[data-pending]', content)) content.innerHTML = pendingHTML();
      return;
    }
    const entry = detail.entry;
    const word = headword(entry);
    const alternate = traditionalOf(entry) !== entry.simplified;
    const related = detail.related || [];
    const characters = entryCharacters(detail);
    const label = `Show ${preferences.traditional ? 'simplified' : 'traditional'} characters`;
    const tabs = {meanings: true, words: related.length > 0, characters: characters.length > 0};
    if (!tabs[state.tab]) state.tab = 'meanings';
    characterResults.clear();
    content.classList.remove('entry-enter');
    content.setAttribute('aria-busy', 'false');
    content.innerHTML = `
      <header class="entry-header">
        <div class="entry-heading">
          <div class="entry-wordline"><h1 class="entry-word${[...word].length >= LONG_HEADWORD ? ' long' : ''}" lang="zh" tabindex="-1" ${scriptData(entry)} data-script-format="clauses">${clausesHTML(word)}</h1>${alternate ? `<button type="button" class="alternate-word" lang="zh" data-toggle-script data-simplified="${escapeHTML(traditionalOf(entry))}" data-traditional="${escapeHTML(entry.simplified)}" title="${label}" aria-label="${escapeHTML(preferences.traditional ? entry.simplified : traditionalOf(entry))}: ${label}">${escapeHTML(preferences.traditional ? entry.simplified : traditionalOf(entry))}</button>` : ''}</div>
          <div class="entry-sound"><span class="entry-pinyin">${escapeHTML(readingText(entry))}</span><button type="button" class="listen-button" data-speak title="Listen" aria-label="Listen"${usualReading(detail) ? '' : ' hidden'}>${icon('audio')}</button>${otherReadingsHTML(detail, 'data-reading')}</div>
        </div>
      </header>
      <div class="entry-tabs" role="tablist" aria-label="Word details">
        ${tabHTML('meanings', 'Meanings')}
        ${tabs.words ? tabHTML('words', 'Words', detail.related_total > related.length ? `${related.length}+` : related.length) : ''}
        ${tabs.characters ? tabHTML('characters', 'Characters', characters.length > 1 ? characters.length : 0) : ''}
      </div>
      <div class="tab-content" id="tab-content" role="tabpanel" aria-labelledby="tab-${state.tab}"></div>`;
    renderTab({animate: false});
    if (animate) restartAnimation(content, 'entry-enter');
    if (!voice) updateVoice();
    updateTitle();
  }

  function tabHTML(name, label, count = 0) {
    const selected = state.tab === name;
    return `<button type="button" class="entry-tab" id="tab-${name}" data-tab="${name}" role="tab" aria-selected="${selected}" aria-controls="tab-content" tabindex="${selected ? 0 : -1}">${label}${name === 'characters' ? ` <small id="character-count"${count ? '' : ' hidden'}>${count || ''}</small>` : count ? ` <small>${count}</small>` : ''}${selected ? '<span class="tab-mark" aria-hidden="true"></span>' : ''}</button>`;
  }

  function selectTab(name) {
    const pane = $('#entry-pane');
    const tabs = $('.entry-tabs');
    if (tabs.getBoundingClientRect().top < pane.getBoundingClientRect().top) {
      pane.scrollTop += tabs.getBoundingClientRect().top - pane.getBoundingClientRect().top;
    }
    state.tab = name;
    go('replace');
    for (const tab of $$('[data-tab]')) {
      const selected = tab.dataset.tab === name;
      tab.setAttribute('aria-selected', String(selected));
      tab.tabIndex = selected ? 0 : -1;
    }
    slideMark($(`[data-tab="${name}"]`));
    $('#tab-content').setAttribute('aria-labelledby', `tab-${name}`);
    renderTab({animate: true});
  }

  function slideMark(tab) {
    const mark = $('.tab-mark');
    if (!mark || mark.parentElement === tab) return;
    const from = mark.getBoundingClientRect();
    mark.getAnimations().forEach(animation => animation.cancel());
    tab.append(mark);
    const to = mark.getBoundingClientRect();
    if (!reducedMotion() && to.width) {
      const animation = motion(mark, [{transform: `translateX(${from.left - to.left}px) scaleX(${from.width / to.width})`}, {transform: 'none'}]);
      animation.finished.then(() => animation.cancel(), () => {});
    }
  }

  function renderTab({animate}) {
    abortPending('character', 'related');
    const container = $('#tab-content');
    const detail = state.detail;
    if (!container || !detail) return;
    if (state.tab === 'meanings') {
      container.innerHTML = meaningsHTML(detail.entry) + examplesHTML(looseExamples(detail.entry));
    } else if (state.tab === 'words') {
      const more = detail.related_total > detail.related.length
        ? '<button type="button" class="text-link more-words" data-more-words>Show more</button>'
        : '';
      container.innerHTML = detail.related.map(wordRowHTML).join('') + more;
    } else {
      showCharacters(entryCharacters(detail), container);
    }
    if (animate) restartAnimation(container, 'tab-enter');
  }

  async function moreWords(button) {
    if (button.getAttribute('aria-disabled') === 'true') return;
    const detail = state.detail;
    const shownRevision = revision.of(detail);
    const controller = begin('related');
    const finish = beginLoading(button, 'Loading more words.', controller.signal, false);
    button.setAttribute('aria-disabled', 'true');
    try {
      const data = await fetchJSON(`/api/entry/${detail.entry.id}?offset=${detail.related.length}`, {signal: controller.signal});
      if (!current('related', controller) || state.detail !== detail || state.selected !== detail.entry.id) return;
      if (revision.of(data) !== shownRevision || data.entry.simplified !== detail.entry.simplified || data.entry.pinyin !== detail.entry.pinyin) return location.reload();
      const first = data.related[0];
      detail.related.push(...data.related);
      detail.related_total = data.related_total;
      button.insertAdjacentHTML('beforebegin', data.related.map(wordRowHTML).join(''));
      const more = detail.related.length < detail.related_total;
      $('[data-tab="words"] small').textContent = `${detail.related.length}${more ? '+' : ''}`;
      if (first) $(`[data-entry="${first.id}"]`, button.parentElement).focus();
      if (!more) button.remove();
      finish('More words loaded.');
    } catch (error) {
      if (current('related', controller) && error.status === 404 && revision.current !== shownRevision) return location.reload();
      if (error.name !== 'AbortError') toast('Couldn’t load more words. Try again.');
    } finally {
      finish();
      button.removeAttribute('aria-disabled');
    }
  }

  const shownPos = senses => new Set(senses.map(sense => sense.pos).filter(Boolean)).size > 1 ? sense => sense.pos || '' : () => '';

  function groupByPos(senses) {
    const groups = [];
    const posOf = shownPos(senses);
    for (const sense of senses) {
      const pos = posOf(sense);
      if (groups.length && groups.at(-1).pos === pos) groups.at(-1).senses.push(sense);
      else groups.push({pos, senses: [sense]});
    }
    return groups;
  }

  function looseExamples(entry) {
    const ids = new Set((entry.senses || []).map(sense => sense.id).filter(Boolean));
    return (entry.examples || []).filter(example => !ids.has(example.sense_id));
  }

  function senseExamplesHTML(own, open) {
    if (!own.length) return '';
    return `<details class="sense-examples"${open ? ' open' : ''}><summary>${own.length === 1 ? 'Example' : 'Examples'}${icon('chevron')}</summary><div class="disclosure-body">${own.map(exampleHTML).join('')}</div></details>`;
  }

  function labelCut(text, labels) {
    const shown = new Set(labels.filter(Boolean).map(label => label.toLowerCase()));
    let cut = 0;
    for (let match; (match = /^\(([^()]*)\)\s*/.exec(text.slice(cut))) && shown.has(match[1].toLowerCase()) && cut + match[0].length < text.length;) cut += match[0].length;
    return cut;
  }

  function referencesHTML(text, refs) {
    let html = '';
    let at = 0;
    for (const {simplified, traditional} of refs) {
      const found = text.indexOf(simplified, at);
      if (found < 0) break;
      const owner = traditional === simplified ? null : {traditional_text: {[simplified]: traditional}};
      html += `${displayText(text.slice(at, found), null, 'rich')}<button type="button" class="word-link" data-lookup="${escapeHTML(preferences.traditional ? traditional : simplified)}" lang="zh" data-script-lookup>${displayText(simplified, owner)}</button>`;
      at = found + simplified.length;
    }
    return html + displayText(text.slice(at), null, 'rich');
  }

  function senseHTML(sense, number, {showSource = false, pos = '', inline = false, notes = false, examples = []} = {}) {
    const tags = sense.tags || [];
    const full = String(sense.text ?? '');
    const cut = labelCut(full, [pos, ...tags]);
    const text = full.slice(cut);
    const traditional = sense.traditional_text?.[full];
    const owner = cut && traditional ? {traditional_text: {[text]: traditional.slice(labelCut(traditional, [pos, ...tags]))}} : sense;
    const labels = (inline && pos ? `<span class="pos">${escapeHTML(pos)}</span> ` : '') + (tags.length ? `<span class="tag">${tags.map(tag => displayText(tag, sense)).join(', ')}</span> ` : '');
    const note = notes && sense.note ? `<p class="sense-note">${displayText(sense.note, sense, 'rich')}</p>` : '';
    const own = sense.id ? examples.filter(example => example.sense_id === sense.id) : [];
    return `<li class="sense"><span class="sense-number">${number}</span><div class="sense-body"><p class="sense-text">${labels}${sense.refs ? referencesHTML(text, sense.refs) : displayText(text, owner, 'rich')}${showSource ? `<span class="sense-source">${escapeHTML(sourceName(sense.source))}</span>` : ''}</p>${note}${senseExamplesHTML(own, number === 1)}</div></li>`;
  }

  function meaningsHTML(entry, {limit = Infinity} = {}) {
    const senses = entry.senses || [];
    const primary = senses[0]?.source;
    const main = senses.filter(sense => sense.source === primary).slice(0, limit);
    const added = senses.filter(sense => sense.source !== primary);
    const full = limit === Infinity;
    const examples = full ? entry.examples || [] : [];
    let number = 0;
    const lone = main.length === 1 && (!full || !added.length);
    const groups = groupByPos(main).map(({pos, senses}) => {
      const inline = senses.length === 1;
      return `<section class="sense-group">${pos && !inline ? `<h3 class="pos">${escapeHTML(pos)}</h3>` : ''}<ol class="senses${lone ? ' lone' : ''}">${senses.map(sense => senseHTML(sense, ++number, {notes: full, examples, pos, inline})).join('')}</ol></section>`;
    }).join('');
    if (!full) return groups;
    const addedPos = shownPos(added);
    return groups + usageHTML(entry, lone) + wiktionaryCreditHTML(entry) + (added.length
      ? `<details class="added-meanings"><summary>More meanings${icon('chevron')}</summary><ol class="senses disclosure-body">${added.map(sense => senseHTML(sense, ++number, {showSource: true, pos: addedPos(sense), inline: true})).join('')}</ol></details>`
      : '');
  }

  // CC BY-SA requires source links; those pages list the authors.
  function wiktionaryCreditHTML(entry) {
    if (!entry.wiktionary_pages?.length) return '';
    const links = entry.wiktionary_pages.map(page => `<a href="https://en.wiktionary.org/wiki/${encodeURIComponent(page.replaceAll(' ', '_'))}" target="_blank" rel="noopener" lang="zh">${escapeHTML(page)}</a>`);
    return `<p class="source-credit">Wiktionary: ${links.join(', ')}</p>`;
  }

  function tatoebaCreditHTML(example) {
    const seen = new Set();
    const links = (example.attribution || []).filter(({author}) => !seen.has(author) && seen.add(author))
      .map(({id, author}) => `<a href="https://tatoeba.org/en/sentences/show/${id}" target="_blank" rel="noopener">${escapeHTML(author)}</a>`);
    return links.length ? `<p class="source-credit">Tatoeba: ${links.join(', ')}</p>` : '';
  }

  function exampleHTML(example) {
    return `<div class="example" title="${escapeHTML(sourceName(example.source))}"><p class="example-chinese" lang="zh">${displayText(example.chinese, example, 'sentence')}</p>${example.pinyin && preferences.examplePinyin ? `<p class="example-pinyin">${examplePinyinHTML(example)}</p>` : ''}<p class="example-english">${escapeHTML(example.english)}</p>${tatoebaCreditHTML(example)}</div>`;
  }

  function examplesHTML(examples) {
    if (!examples?.length) return '';
    const rest = examples.slice(3);
    return `<section class="examples" aria-label="Examples"><h3 class="section-title">Examples</h3>${examples.slice(0, 3).map(exampleHTML).join('')}${rest.length ? `<details class="more-examples"><summary>${rest.length} more ${rest.length === 1 ? 'example' : 'examples'}${icon('chevron')}</summary><div class="disclosure-body">${rest.map(exampleHTML).join('')}</div></details>` : ''}</section>`;
  }

  function wordRowHTML(entry) {
    return `<button type="button" class="word-row" data-entry="${entry.id}">${rowContentHTML(entry)}</button>`;
  }

  // Use this word's reading and meaning; keep the row's shape while loading.
  function characterRowHTML({character: char, reading}, loading) {
    const {data, error} = loading ? {} : characterResults.get(char) || {};
    const item = data?.character;
    const entries = data?.entries || [];
    // words before names with the same reading (hang2, Hang2)
    const entry = entries.find(summary => summary.pinyin === reading)
      || entries.find(summary => summary.pinyin.toLowerCase() === reading?.toLowerCase())
      || entries[0];
    const lines = entry ? entry.gloss.slice(0, CHARACTER_SENSES).map(text => displayText(text, entry, 'gloss'))
      : item?.definition ? [escapeHTML(item.definition)]
        : error ? (error.retry ? [] : [escapeHTML(error.message)]) : ['<span class="skeleton"></span>'];
    const head = `<span class="row-head"><span class="result-word" lang="zh">${escapeHTML(char)}</span><span class="result-pinyin">${escapeHTML(pinyinText(entry?.pinyin || reading || '', char))}</span></span><span class="character-senses">${lines.map(line => `<span>${line}</span>`).join('')}</span>`;
    const picture = item?.strokes?.length
      ? `<button type="button" class="stroke-picture" data-play="${escapeHTML(char)}" aria-label="Play the stroke order of ${escapeHTML(char)}"><span class="stroke-art">${strokeSVG(item)}</span>${icon('play')}</button>`
      : `<span class="stroke-picture stroke-placeholder" lang="zh">${data || error ? escapeHTML(char) : ''}</span>`;
    return `<li class="character-row"${data || error ? '' : ' aria-busy="true"'}>${picture}<div class="character-text">${entry ? `<button type="button" class="word-row" data-entry="${entry.id}">${head}</button>` : `<div class="word-row">${head}</div>`}</div></li>`;
  }

  function characterListHTML(list, loading = false) {
    const failed = loading ? null : list.map(item => characterResults.get(item.character)?.error).find(error => error?.retry);
    const note = failed ? `<p class="character-error">${escapeHTML(failed.message)} <button type="button" class="text-link" data-retry-characters>Try again</button></p>` : '';
    return `<ul class="character-rows${list.length === 1 ? ' single' : ''}">${list.map(item => characterRowHTML(item, loading)).join('')}</ul>${note}`;
  }

  // per view, so a row never depends on what the cache still holds
  const characterResults = new Map();

  async function fetchCharacter(char) {
    try {
      characterResults.set(char, {data: await characters.get(char)});
    } catch (error) {
      characterResults.set(char, {error: error.status === 404 ? {message: 'No details for this character.'} : {message: error.message, retry: true}});
    }
  }

  async function retryCharacters() {
    const failed = [...characterResults].filter(([, result]) => result.error).map(([character]) => character);
    for (const character of failed) characterResults.delete(character);
    await Promise.all(failed.map(fetchCharacter));
    const detail = state.detail;
    const container = $('#tab-content');
    if (!detail || !container || state.tab !== 'characters') return;
    await showCharacters(entryCharacters(detail), container);
    if (failed.some(character => characterResults.get(character).error?.retry)) retryFocus('characters');
  }

  async function showCharacters(list, container) {
    const controller = begin('character');
    const missing = list.filter(item => !characterResults.has(item.character));
    let finish = () => {};
    if (missing.some(item => !characters.peek(item.character))) {
      container.innerHTML = characterListHTML(list, true);
      finish = beginLoading($('.character-rows', container), 'Loading character details.', controller.signal, false);
    }
    await Promise.all(missing.map(item => fetchCharacter(item.character)));
    finish();
    if (current('character', controller) && container.isConnected) container.innerHTML = characterListHTML(list);
  }

  const medianLength = median => median.slice(1).reduce((sum, point, index) => sum + Math.hypot(point[0] - median[index][0], point[1] - median[index][1]), 0);

  function strokeSVG(character, animate = false) {
    const paths = character.strokes || [];
    const medians = character.medians || [];
    const id = `stroke-${character.character.codePointAt(0)}`;
    const body = paths.map((path, index) => {
      if (animate && medians[index]?.length) {
        const length = medianLength(medians[index]);
        const line = medians[index].map((point, i) => `${i ? 'L' : 'M'} ${point[0]} ${point[1]}`).join(' ');
        return `<path class="stroke-median" data-stroke="${index}" data-length="${length}" clip-path="url(#${id}-${index})" d="${line}" stroke-dasharray="${length} ${length}" stroke-dashoffset="${length}" visibility="hidden"/>`;
      }
      return `<path${animate ? ` data-stroke="${index}" visibility="hidden"` : ''} d="${escapeHTML(path)}"/>`;
    }).join('');
    const ghost = animate ? `<g class="stroke-background">${paths.map(path => `<path d="${escapeHTML(path)}"/>`).join('')}</g><defs>${paths.map((path, index) => `<clipPath id="${id}-${index}"><path d="${escapeHTML(path)}"/></clipPath>`).join('')}</defs>` : '';
    return `<svg viewBox="0 0 1024 1024" aria-hidden="true"><g transform="translate(0 900) scale(1 -1)">${ghost}${body}</g></svg>`;
  }

  const strokeRuns = new WeakMap();
  const easeInOut = t => t < .5 ? 2 * t * t : 1 - (2 - 2 * t) ** 2 / 2;

  // Only write dash offsets to avoid layout during playback.
  function animateStrokes(button) {
    const item = characterResults.get(button.dataset.play)?.data?.character;
    if (!item?.strokes?.length) return;
    const run = {};
    strokeRuns.set(button, run);
    $('.stroke-art', button).innerHTML = strokeSVG(item, true);
    button.classList.add('playing');
    const reduced = reducedMotion();
    let end = 0;
    const steps = $$('[data-stroke]', button).map(path => {
      const length = Number(path.dataset.length) || 0;
      const duration = reduced ? 0 : 260 + length * .45;
      const step = {path, length, begin: end, duration, shown: 0};
      end += duration + (reduced ? 450 : 90);
      return step;
    });
    let first = null;
    const frame = now => {
      if (strokeRuns.get(button) !== run || !button.isConnected) return;
      first ??= now;
      const elapsed = now - first;
      for (const step of steps) {
        const progress = step.duration ? Math.min(1, Math.max(0, (elapsed - step.begin) / step.duration)) : Number(elapsed >= step.begin);
        if (progress === step.shown) continue;
        if (!step.shown) step.path.setAttribute('visibility', 'visible');
        step.shown = progress;
        if (step.length) step.path.style.strokeDashoffset = step.length * (1 - easeInOut(progress));
      }
      if (elapsed < end) requestAnimationFrame(frame);
      else button.classList.remove('playing');
    };
    requestAnimationFrame(frame);
  }

  const VOICE_PREFERENCE = [/google/i, /natural|neural/i, /premium/i, /enhanced/i, /cmn[-_]cn[-_]x/i];
  // Apple's robotic multi-language voices (Eddy, Flo and others), named "Eddy (Chinese (China mainland))" in Chrome
  const ROBOTIC_VOICE = /eloquence|\(.*\(.*\)\)/i;
  let voice = null;

  function pickVoice() {
    if (!('speechSynthesis' in window)) return null;
    const rank = item => {
      const region = /^(zh[-_]CN|cmn)/i.test(item.lang) ? 0 : /^(yue|zh[-_](HK|MO))/i.test(item.lang) ? 2 : 1;
      const natural = VOICE_PREFERENCE.findIndex(pattern => pattern.test(`${item.name} ${item.voiceURI}`));
      const robotic = ROBOTIC_VOICE.test(`${item.name} ${item.voiceURI}`) ? 100 : 0;
      return robotic + region * 10 + (natural < 0 ? VOICE_PREFERENCE.length : natural);
    };
    const chinese = speechSynthesis.getVoices().filter(item => item.localService === true && /^(zh|cmn|yue)([-_]|$)/i.test(item.lang));
    return chinese.sort((a, b) => rank(a) - rank(b))[0] || null;
  }

  // voices read the usual reading, so other readings get no button
  const usualReading = (detail = state.detail) => Boolean(detail && (detail.readings?.[0]?.id ?? detail.entry.id) === detail.entry.id);

  // browsers empty the list briefly while reloading it, so keep the last voice meanwhile
  function updateVoice() {
    const reloading = 'speechSynthesis' in window && !speechSynthesis.getVoices().length;
    voice = pickVoice() || (reloading ? voice : null);
    document.documentElement.classList.toggle('has-voice', Boolean(voice));
  }

  function stopPlayback() {
    if ('speechSynthesis' in window) speechSynthesis.cancel();
    $('[data-speak]')?.classList.remove('playing');
  }

  function speak() {
    updateVoice();
    if (!voice || !usualReading()) return;
    stopPlayback();
    const button = $('[data-speak]');
    const utterance = new SpeechSynthesisUtterance(state.detail.entry.simplified);
    utterance.voice = voice;
    utterance.lang = voice.lang;
    utterance.onstart = () => button?.classList.add('playing');
    utterance.onend = () => button?.classList.remove('playing');
    utterance.onerror = event => {
      button?.classList.remove('playing');
      if (!['interrupted', 'canceled'].includes(event.error)) toast('Pronunciation could not be played. Try again.');
    };
    speechSynthesis.speak(utterance);
  }

  function closeLookup(restoreFocus = false) {
    abortPending('peek');
    state.peek.returnTo?.classList.remove('looked-up');
    if (popup.hidden) return;
    present(popup, false);
    if (restoreFocus) {
      const target = state.peek.returnTo?.isConnected ? state.peek.returnTo : state.peek.anchor;
      if (target?.isConnected) target.focus({preventScroll: true});
    }
  }

  popup.addEventListener('focusout', event => {
    if (event.relatedTarget && !popup.contains(event.relatedTarget)) closeLookup();
  });

  function positionLookup() {
    popup.removeAttribute('style');
    if (popup.hidden || mobileView()) return;
    const bounds = state.peek.anchor?.isConnected ? state.peek.anchor.getBoundingClientRect() : state.peek.bounds;
    const width = Math.min(380, innerWidth - 24);
    popup.style.width = `${width}px`;
    const left = bounds ? bounds.left : innerWidth - width - 12;
    popup.style.left = `${Math.max(12, Math.min(left, innerWidth - width - 12))}px`;
    const below = bounds ? innerHeight - bounds.bottom - 20 : innerHeight - 24;
    const above = bounds ? bounds.top - $('.app-header').getBoundingClientRect().bottom - 20 : 0;
    const placeBelow = below >= LOOKUP_MAX_HEIGHT || below >= above;
    popup.style.maxHeight = `${Math.min(LOOKUP_MAX_HEIGHT, placeBelow ? below : above)}px`;
    popup.style.top = placeBelow ? `${bounds ? bounds.bottom + 8 : 12}px` : 'auto';
    popup.style.bottom = placeBelow ? 'auto' : `${innerHeight - bounds.top + 8}px`;
  }

  function peekHeading(title, extra = '') {
    return `<div class="peek-heading"><div><h2 lang="zh">${escapeHTML(title)}</h2>${extra}</div><button type="button" class="icon-button small" data-close-lookup aria-label="Close lookup" title="Close">${icon('close')}</button></div>`;
  }

  function markEdges(body) {
    body.classList.toggle('more-above', body.scrollTop > 1);
    body.classList.toggle('more-below', body.scrollTop + body.clientHeight < body.scrollHeight - 1);
  }

  function showPeek(html) {
    const swapping = !popup.hidden;
    popup.innerHTML = html;
    present(popup, true);
    positionLookup();
    const body = $('.peek-body', popup);
    if (body) {
      markEdges(body);
      body.addEventListener('scroll', () => markEdges(body), {passive: true});
      body.addEventListener('keydown', event => {
        if (event.target !== body || !['Home', 'End'].includes(event.key)) return;
        event.preventDefault();
        body.scrollTo({top: event.key === 'Home' ? 0 : body.scrollHeight, behavior: 'instant'});
      });
    }
    if (swapping) restartAnimation(popup, 'content-enter');
    if (state.peek.keyboard) $('[data-close-lookup]', popup)?.focus({preventScroll: true});
  }

  const searchForHTML = word => `<button type="button" class="peek-open" data-search-text="${escapeHTML(word)}">Search for “${escapeHTML(word)}”${icon('chevron')}</button>`;
  let peekEntries = [];
  let peekRevision = null;

  async function showLookup(word, anchor, keyboard) {
    state.peek.keyboard = keyboard;
    if (!anchor.closest('#lookup-popover')) {
      state.peek.returnTo?.classList.remove('looked-up');
      state.peek.returnTo = anchor;
      anchor.classList.add('looked-up');
    }
    state.peek.anchor = anchor;
    state.peek.bounds = anchor.getBoundingClientRect();
    // the card waits for its meanings so it never grows after appearing
    const controller = begin('peek');
    const finish = beginLoading(popup, 'Loading meanings.', controller.signal);
    try {
      const params = new URLSearchParams({q: word, mode: 'chinese', offset: 0, limit: LOOKUP_RESULT_LIMIT});
      const data = await fetchJSON(`/api/search?${params}`, {signal: controller.signal});
      if (!current('peek', controller)) return;
      peekEntries = data.results;
      peekRevision = revision.current;
      const exact = data.results.filter(item => data.exact.includes(item.id) || item.simplified === word || item.traditional === word);
      if (!exact.length) {
        const byId = new Map(data.results.map(item => [item.id, item]));
        const parts = (data.segments || []).map(segment => byId.get(segment.ids[0])).filter(Boolean);
        const words = parts.length ? parts : data.results.slice(0, 4);
        showPeek(`${peekHeading(word)}<p class="hint">${parts.length ? 'No entry for the whole text. Its words:' : 'No entry for this exact text.'}</p>${words.length ? `<div class="peek-alternatives">${words.map(item => `<button type="button" class="reading-choice" data-peek-entry="${item.id}"><span lang="zh">${escapeHTML(headword(item))}</span> ${escapeHTML(readingText(item))}</button>`).join('')}</div>` : ''}${searchForHTML(word)}`);
        return;
      }
      await renderPeek(exact[0].id, controller, finish);
    } catch (error) {
      if (current('peek', controller) && error.name !== 'AbortError') {
        showPeek(`${peekHeading(word)}<p class="hint">${escapeHTML(error.message)}</p>${searchForHTML(word)}`);
      }
    } finally {
      finish('Lookup updated.');
    }
  }

  async function renderPeek(id, controller = begin('peek'), finish = beginLoading(popup, 'Loading meanings.', controller.signal)) {
    const known = peekEntries.find(item => item.id === id);
    const requestedRevision = peekRevision;
    try {
      const data = await entries.get(id);
      if (!current('peek', controller)) return;
      const entry = data.entry;
      if (!known || known.simplified !== entry.simplified || known.pinyin !== entry.pinyin) return location.reload();
      peekEntries = [entry, ...(data.readings || []).map(reading => ({simplified: entry.simplified, ...reading}))];
      peekRevision = revision.current;
      const pinyin = `<span>${escapeHTML(readingText(entry))}</span>${otherReadingsHTML(data, 'data-peek-entry')}`;
      finish('Meanings ready.');
      showPeek(`${peekHeading(headword(entry), `<div class="peek-pinyin">${pinyin}</div>`)}<div class="peek-body" tabindex="0" role="region" aria-label="Meanings">${meaningsHTML(entry, {limit: 5})}</div><button type="button" class="peek-open" data-open-entry="${entry.id}" data-open-lookup="${escapeHTML(headword(entry))}">Open full entry${icon('chevron')}</button>`);
    } catch (error) {
      if (current('peek', controller) && error.status === 404 && revision.current !== requestedRevision) return location.reload();
      throw error;
    } finally {
      finish();
    }
  }

  let drag = null;
  popup.addEventListener('pointerdown', event => {
    if (!mobileView() || !event.isPrimary || event.target.closest('button') || !(event.target === popup || event.target.closest('.peek-heading'))) return;
    drag = {id: event.pointerId, y: event.clientY, time: event.timeStamp, distance: 0};
    popup.setPointerCapture(event.pointerId);
    hold(popup);
  });
  popup.addEventListener('pointermove', event => {
    if (drag?.id !== event.pointerId) return;
    drag.distance = Math.max(0, event.clientY - drag.y);
    popup.style.transform = `translateY(${drag.distance}px)`;
  });
  for (const type of ['pointerup', 'pointercancel']) {
    popup.addEventListener(type, event => {
      if (drag?.id !== event.pointerId) return;
      const {distance, time} = drag;
      drag = null;
      const flick = distance / Math.max(1, event.timeStamp - time) > SHEET_FLICK;
      if (type === 'pointerup' && (distance > SHEET_CLOSE_DISTANCE || flick)) closeLookup(popup.contains(document.activeElement));
      else present(popup, true);
    });
  }

  if ('speechSynthesis' in window) {
    speechSynthesis.addEventListener('voiceschanged', updateVoice);
    addEventListener('pageshow', updateVoice);
    document.addEventListener('visibilitychange', () => document.hidden || updateVoice());
    // Poll briefly because some Safari versions never fire voiceschanged.
    let checks = 6;
    const retry = () => {
      updateVoice();
      if (!speechSynthesis.getVoices().length && --checks > 0) setTimeout(retry, 500);
    };
    retry();
  }
  return {prefetchCharacters, summaryOf, showEntry, selectTab, renderEntry, renderTab, moreWords, closeLookup, positionLookup, showLookup, renderPeek, animateStrokes, retryCharacters, stopPlayback, speak};
}

export {initializeEntry};
