import {preferences, HAN, escapeHTML, state, SOURCE_NAMES, LONG_HEADWORD} from "/shared.js";


const TONE_MARKS = {a: 'āáǎà', e: 'ēéěè', i: 'īíǐì', o: 'ōóǒò', u: 'ūúǔù', ü: 'ǖǘǚǜ'};
const TONE_ACCENTS = '\u0304\u0301\u030c\u0300';

const SYLLABLE = /^(?:[bpmfdtnlgkhjqxrzcsyw]|[zcs]h)?(?:[iu]?a(?:o|i|n|ng)?|i?ou|[iuü]?e(?:i|n|ng|r)?|[iu]?o(?:ng)?|i(?:a(?:o|n|ng)?|e|u|n|ng|ong)?|u(?:a(?:i|n|ng)?|e|i|n|o)?|ü(?:e|n|an)?|ng|n|m|r)$/i;

function numberedSyllables(text) {
  const pieces = String(text).match(/[A-Za-zÜü:]+[0-5]?/g);
  if (!pieces || pieces.join('') !== text) return null;
  // Sources use lyu3 for lǚ and nar3 for toned erhua.
  const syllables = pieces.map(piece => parseSyllable(piece.replace(/^([lnLN])yu/, '$1ü')));
  const real = letters => SYLLABLE.test(letters) || (/.r$/i.test(letters) && SYLLABLE.test(letters.slice(0, -1)));
  return syllables.every(syllable => syllable && real(syllable.letters.replace(/ü/gi, 'u'))) ? syllables : null;
}

function parseSyllable(token) {
  let markedTone = 5;
  const plain = String(token).normalize('NFD').replace(new RegExp(`[${TONE_ACCENTS}]`, 'g'), mark => {
    markedTone = TONE_ACCENTS.indexOf(mark) + 1;
    return '';
  }).normalize('NFC');
  const match = plain.match(/^([A-Za-zÜü:]+?)([0-5])?$/);
  if (!match) return null;
  // Toneless capitals are Latin letters, as in V C R.
  if (!match[2] && markedTone === 5 && match[1].length <= 3 && match[1] === match[1].toUpperCase()) return null;
  const letters = match[1].replace(/u:/g, 'ü').replace(/U:/g, 'Ü').replace(/v/g, 'ü').replace(/V/g, 'Ü');
  if (!/[aeiouüAEIOUÜ]/.test(letters) && !/^(r|m|n|ng|hm|hng)$/i.test(letters)) return null;
  return {letters, tone: Number(match[2] || markedTone) || 5};
}

function markSyllable(letters, tone) {
  if (tone >= 5) return letters;
  const lower = letters.toLowerCase();
  let index = lower.indexOf('a');
  if (index < 0) index = lower.indexOf('e');
  if (index < 0 && lower.includes('ou')) index = lower.indexOf('o');
  if (index < 0) {
    for (let i = lower.length - 1; i >= 0; i--) {
      if ('iouü'.includes(lower[i])) {
        index = i;
        break;
      }
    }
  }
  if (index < 0) {
    const nasal = letters.search(/[mn]/i);
    return nasal < 0 ? letters : (letters.slice(0, nasal + 1) + TONE_ACCENTS[tone - 1] + letters.slice(nasal + 1)).normalize('NFC');
  }
  let mark = TONE_MARKS[lower[index]][tone - 1];
  if (letters[index] !== lower[index]) mark = mark.toUpperCase();
  return letters.slice(0, index) + mark + letters.slice(index + 1);
}

// Numbered erhua puts the tone after the joined r (huar1).
const isErhua = syllable => syllable.letters.toLowerCase() === 'r';
const joinErhua = (previous, syllable) => ({letters: previous.letters + syllable.letters, tone: previous.tone});

const CLAUSE_MARKS = {'，': ',', ',': ',', '、': ',', '；': ';', ';': ';', '：': ':', ':': ':', '。': '.', '！': '!', '？': '?'};

// Preserve source word breaks and hyphens.
function writtenText(written) {
  const numbered = preferences.pinyin === 'numbered';
  const words = [[]];
  for (const [, letters, tone, join] of written.matchAll(/([A-Za-zÜü]+?)([1-5])|([ -])/g)) {
    if (join === ' ') words.push([]);
    else if (join) words.at(-1).push(join);
    else {
      const syllable = {letters, tone: Number(tone)};
      const last = words.at(-1).at(-1);
      if (numbered && isErhua(syllable) && last?.letters) words.at(-1)[words.at(-1).length - 1] = joinErhua(last, syllable);
      else words.at(-1).push(syllable);
    }
  }
  return words.map(items => items.map((item, index) => {
    if (typeof item === 'string') return item;
    if (numbered) return item.letters + item.tone;
    const marked = markSyllable(item.letters, item.tone);
    // Vowel-start syllables need an apostrophe (Xī'ān).
    return index && typeof items[index - 1] !== 'string' && !isErhua(item) && /^[aeoāáǎàēéěèōóǒò]/i.test(marked) ? '\u2019' + marked : marked;
  }).join('')).join(' ');
}

// Prefer source word breaks; otherwise use capitals and server boundaries.
function pinyinText(text, word = '', wordStarts = 0, written = '') {
  const shown = pinyinBody(text, word, wordStarts, written);
  // Ordinal 第 + numeral takes a hyphen.
  return /^第[〇零一二两三四五六七八九十百千万]/.test(word) ? shown.replace(/^([Dd](?:ì|i4))[\u2019 -]?(?=\S)/, '$1-') : shown;
}

function pinyinBody(text, word, wordStarts, written) {
  if (written) return writtenText(written);
  const parts = String(text ?? '').trim().split(/\s+/).filter(Boolean).map(token => ({token, syllable: parseSyllable(token)}));
  if (preferences.pinyin === 'numbered') {
    const items = [];
    for (const item of parts) {
      const last = items.at(-1);
      if (item.syllable && last?.syllable && isErhua(item.syllable)) last.syllable = joinErhua(last.syllable, item.syllable);
      else items.push(item);
    }
    return items.map(({token, syllable}) => syllable ? syllable.letters + syllable.tone : token).join(' ');
  }
  // Restore punctuation omitted by sources from the headword.
  const breaks = new Map();
  const characters = [...String(word)];
  if (characters.filter(char => HAN.test(char)).length === parts.length) {
    let index = 0;
    for (const char of characters) {
      if (HAN.test(char)) index++;
      else if (CLAUSE_MARKS[char] && index) breaks.set(index - 1, CLAUSE_MARKS[char]);
    }
  }
  const name = /^[A-ZÜ]/.test(parts[0]?.syllable?.letters ?? '');
  const words = [];
  let current = -1;
  for (const [index, {token, syllable}] of parts.entries()) {
    if (!syllable) {
      words.push(token);
      current = -1;
      continue;
    }
    const marked = markSyllable(syllable.letters, syllable.tone);
    const erhua = syllable.letters.toLowerCase() === 'r' && current >= 0;
    const starts = (wordStarts >> index) & 1;
    if (current >= 0 && !starts && (erhua || !/^[A-ZÜ]/.test(syllable.letters))) {
      const apostrophe = !erhua && /^[aeoāáǎàēéěèōóǒò]/i.test(marked) ? '\u2019' : '';
      words[current] += apostrophe + marked;
    } else {
      current = words.push(starts && name ? marked[0].toUpperCase() + marked.slice(1) : marked) - 1;
    }
    if (breaks.has(index)) {
      words[current] += breaks.get(index);
      current = -1;
    }
  }
  return words.join(' ');
}

function sentencePinyinHTML(text, bold = new Set()) {
  const numbered = preferences.pinyin === 'numbered';
  const words = [];
  for (const [index, token] of String(text ?? '').trim().split(/\s+/).entries()) {
    const [, head, core, tail] = token.match(/^([("“‘'"]*)(.*?)([,.!?;:，。！？；：、)"”’']*)$/);
    const last = words.at(-1);
    if (/^r[05]?$/i.test(core) && last && !head) {
      if (last.syllables) {
        last.syllables.push({letters: 'r', tone: 5});
        last.tail += tail;
      } else last.token += 'r' + tail;
      continue;
    }
    const syllables = numbered || /[0-5]/.test(core) ? numberedSyllables(core) : null;
    words.push({token, head, tail, syllables, bold: bold.has(index)});
  }
  const html = words.map(({token, head, tail, syllables, bold}) => {
    const items = [];
    for (const syllable of syllables || []) {
      if (items.length && isErhua(syllable)) items[items.length - 1] = joinErhua(items.at(-1), syllable);
      else items.push(syllable);
    }
    const text = syllables
      ? head + items.map(({letters, tone}) => numbered ? letters + tone : markSyllable(letters, tone)).join(numbered ? ' ' : '') + tail
      : token.normalize('NFC');
    return bold ? `<b>${escapeHTML(text)}</b>` : escapeHTML(text);
  });
  return html.join(' ').replace(/\s+([,.!?;:，。！？；：、)”’—])/g, '$1').replace(/([(“‘—])\s+/g, '$1');
}

const readingText = (entry, reading = entry) => pinyinText(reading.pinyin, entry.simplified, reading.word_starts, reading.written);
const traditionalOf = entry => Array.isArray(entry.traditional) ? entry.traditional[0] || entry.simplified : entry.traditional || entry.simplified;
const headword = entry => preferences.traditional ? traditionalOf(entry) : entry.simplified;
const entryCharacters = (detail = state.detail) => {
  if (!detail) return [];
  const word = [...headword(detail.entry)];
  const syllables = detail.entry.pinyin.split(' ');
  const aligned = syllables.length === word.length;
  // Unaligned pinyin is safe only for single-reading characters.
  return [...new Set(word)].map(char => [char, detail.characters?.find(item => item.character === char)]).filter(([, item]) => item)
    .map(([char, item]) => ({character: char, reading: aligned ? syllables[word.indexOf(char)] : item.pinyin.length === 1 ? item.pinyin[0] : undefined}));
};
const scriptData = entry => `data-simplified="${escapeHTML(entry.simplified)}" data-traditional="${escapeHTML(traditionalOf(entry))}"`;
const sourceName = id => SOURCE_NAMES[id] || id;
const plainGloss = text => String(text ?? '').replace(/\[[A-Za-zÜü: ]+[0-5][A-Za-zÜü0-5: ]*\]/g, '');

function richText(text) {
  let html = escapeHTML(text);
  html = html.replace(/\[([A-Za-zÜü: ]+[0-5][A-Za-zÜü0-5: ]*)\]/g, (match, pinyin) => ` <span class="ref-pinyin">${escapeHTML(pinyinText(pinyin))}</span>`);
  // Convert note pinyin only when every syllable is valid.
  html = html.replace(/(?<![A-Za-z0-9])[a-zü:]+[1-5](?:[a-zü:]+[0-5])*(?![A-Za-z0-9])/g, run => {
    const syllables = numberedSyllables(run);
    return syllables ? pinyinText(syllables.map(({letters, tone}) => letters + tone).join(' ')) : run;
  });
  html = html.replace(/\p{Script=Han}+/gu, sentenceHTML);
  return html.replace(/\[([^\]]+)\]/g, '<span class="note">[$1]</span>');
}

const segmenter = typeof Intl.Segmenter === 'function' ? new Intl.Segmenter('zh', {granularity: 'word'}) : null;

const CLOSING = /^[，。、；：！？）」』”’》〉】…,.;:!?)]+/u;

const openForms = () => {
  const entry = state.detail?.entry;
  return entry ? [...new Set([entry.simplified, entry.traditional].flat().filter(Boolean))].sort((a, b) => b.length - a.length) : [];
};

const wordRanges = (text, word) => {
  const ranges = [];
  for (let at = word ? text.indexOf(word) : -1; at >= 0; at = text.indexOf(word, at + word.length)) ranges.push([at, at + word.length]);
  return ranges;
};

function boldHTML(text, offset, ranges) {
  let html = '';
  let at = 0;
  for (const [start, end] of ranges) {
    const from = Math.max(start - offset, at);
    const to = Math.min(end - offset, text.length);
    if (from >= to) continue;
    html += `${escapeHTML(text.slice(at, from))}<b>${escapeHTML(text.slice(from, to))}</b>`;
    at = to;
  }
  return html + escapeHTML(text.slice(at));
}

// Keep the headword bold even when the segmenter splits it.
function sentenceHTML(text) {
  text = String(text);
  const ranges = openForms().flatMap(form => wordRanges(text, form)).sort((a, b) => a[0] - b[0]);
  const units = [];
  for (const part of segmenter ? segmenter.segment(text) : [{segment: text, index: 0, isWordLike: true}]) {
    if (units.length && ranges.some(([start, end]) => start < part.index && end > part.index)) {
      units.at(-1).segment += part.segment;
      units.at(-1).isWordLike = true;
    } else units.push({segment: part.segment, index: part.index, isWordLike: part.isWordLike});
  }
  const html = [];
  let stop = true;
  let lastWord = false;
  for (const {segment, index, isWordLike} of units) {
    if (ranges.some(([start, end]) => start === index && end === index + segment.length)) {
      html.push(`<b>${escapeHTML(segment)}</b>`);
      lastWord = true;
      continue;
    }
    if (isWordLike && HAN.test(segment)) {
      html.push(`<button type="button" class="word-link" data-lookup="${escapeHTML(segment)}" lang="zh" tabindex="${stop ? 0 : -1}">${boldHTML(segment, index, ranges)}</button>`);
      stop = false;
      lastWord = true;
      continue;
    }
    const closing = lastWord ? segment.match(CLOSING)?.[0] ?? '' : '';
    if (closing) html.push(`<span class="no-break">${html.pop()}${escapeHTML(closing)}</span>`);
    html.push(boldHTML(segment.slice(closing.length), index + closing.length, ranges));
    lastWord = false;
  }
  return `<span data-lookup-words>${html.join('')}</span>`;
}

// Syllable positions are reliable only with one syllable per character.
function examplePinyinHTML(example) {
  const ranges = wordRanges(example.chinese, state.detail?.entry.simplified);
  const bold = new Set();
  let characters = 0;
  let at = 0;
  for (const char of example.chinese) {
    if (HAN.test(char)) {
      if (ranges.some(([start, end]) => at >= start && at < end)) bold.add(characters);
      characters++;
    }
    at += char.length;
  }
  const lined = example.pinyin.trim().split(/\s+/).length === characters;
  return sentencePinyinHTML(example.pinyin, lined ? bold : undefined);
}

// Keep both scripts in the node to preserve open examples on a switch.
const clausesHTML = text => String(text).split(/(?<=[，,；;])/).map(clause => `<span class="clause">${escapeHTML(clause)}</span>`).join('');
const scriptFormats = {plain: escapeHTML, rich: richText, sentence: sentenceHTML, gloss: text => escapeHTML(plainGloss(text)), clauses: clausesHTML};

function displayText(text, owner, format = 'plain') {
  const render = scriptFormats[format];
  const alternatives = owner?.traditional_text;
  if (!alternatives || !Object.hasOwn(alternatives, text)) return render(text);
  const traditional = alternatives[text];
  return `<span data-script-format="${format}" data-simplified="${escapeHTML(text)}" data-traditional="${escapeHTML(traditional)}">${render(preferences.traditional ? traditional : text)}</span>`;
}

function rowHeadHTML(word, pinyin, meaning, attributes = '') {
  return `<span class="row-head"><span class="result-word${[...word].length >= LONG_HEADWORD ? ' long' : ''}" lang="zh"${attributes}>${escapeHTML(word)}</span><span class="result-pinyin">${escapeHTML(pinyin)}</span></span><span class="result-gloss">${meaning}</span>`;
}

const glossHTML = entry => entry.gloss.map(text => `<span class="gloss-sense">${displayText(text, entry, 'gloss')}</span>`).join(' ');

function rowContentHTML(entry) {
  return rowHeadHTML(headword(entry), readingText(entry), glossHTML(entry), ` ${scriptData(entry)}`);
}

export {
  entryCharacters, headword, scriptData, clausesHTML, readingText, displayText, traditionalOf,
  sourceName, examplePinyinHTML, rowContentHTML, pinyinText, scriptFormats,
};
