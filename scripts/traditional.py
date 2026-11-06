"""Public phrase spellings with longest-match conversion and neighbour fallback."""
from collections import defaultdict
from itertools import product
from pathlib import Path

# characters whose default form is kept as listed, shared with src/traditional.rs
KEPT_FILE = Path(__file__).resolve().parents[1] / 'data/traditional-kept.tsv'
KEPT = dict(line.split('\t') for line in KEPT_FILE.read_text(encoding='utf-8').splitlines()
            if line.strip() and not line.startswith('#'))


class Traditional:
    def __init__(self, words, variants, counts, equivalents=()):
        # only explicit cc variant links join glyphs; 制/製 and 松/鬆 aren't interchangeable
        links = defaultdict(set)
        for a, b in equivalents:
            links[a].add(b)
            links[b].add(a)
        self.glyphs = {}
        for char in links:
            seen, pending = {char}, [char]
            while pending:
                for other in links[pending.pop()] - seen:
                    seen.add(other)
                    pending.append(other)
            self.glyphs[char] = sorted(seen - {char})
        self.attested = {key: list(word['traditional']) for key, word in words.items()}
        weights = defaultdict(lambda: [0, 0])
        contexts = defaultdict(lambda: defaultdict(int))
        spellings = {}
        self.alternatives = defaultdict(list)
        singles = {}
        self.readings = defaultdict(dict)
        self.counts = counts
        for key, word in words.items():
            text, forms = key[0], word['traditional']
            if not forms:
                continue
            target = forms[0]
            for form in forms:
                if form not in self.alternatives[text]:
                    self.alternatives[text].append(form)
            if len(text) == 1:
                singles.setdefault(text, target)
                self.readings[text][key[1]] = list(forms)
                continue
            spellings.setdefault(text, target)
            if len(text) == len(target):
                for position, pair in enumerate(zip(text, target)):
                    weight = weights[pair]
                    # one common phrase mustn't decide every use of an ambiguous glyph
                    weight[0] += 1
                    weight[1] += counts.get(text, 0)
                    for neighbour in (position - 1, position + 1):
                        if 0 <= neighbour < len(text):
                            contexts[pair[0], neighbour - position, text[neighbour]][pair[1]] += 1
        best = {}
        ambiguous = set()
        for (char, target), weight in sorted(weights.items()):
            if char in best and best[char][0] != target:
                ambiguous.add(char)
            if char not in best or weight > best[char][1]:
                best[char] = target, weight
        # a lone 制 can't say whether it's 製 or 制; contexts decide
        self.characters = {char: forms[0] for char, forms in variants.items()}
        self.characters.update(singles)
        self.characters.update({char: target for char, (target, _) in best.items()})
        self.characters.update(KEPT)
        self.contexts = {key: forms for key, forms in contexts.items() if key[0] in ambiguous}
        self.weights = weights
        # identity mappings keep traditional phrases such as 皇后 as they are
        for word in words.values():
            for form in word['traditional']:
                if len(form) > 1:
                    spellings.setdefault(form, form)
        # unambiguous prefixes mustn't swallow a phrase boundary
        self.phrases = {text: target for text, target in spellings.items()
                        if self.fallback(text) != target or any(c in ambiguous for c in text)
                        or len(self.alternatives[text]) > 1}
        self.max_chars = max(map(len, self.phrases), default=1)

    def fallback(self, text):
        return ''.join(self.characters.get(char, char) for char in text)

    def character(self, text, position, forms=()):
        char = text[position]
        evidence = defaultdict(int)
        for neighbour in (position - 1, position + 1):
            if 0 <= neighbour < len(text):
                for target, count in self.contexts.get((char, neighbour - position, text[neighbour]), {}).items():
                    if not forms or target in forms:
                        evidence[target] = max(evidence[target], count)
        # strongest bigram either side; ties as in the plain fallback
        if evidence:
            return max(sorted(evidence), key=lambda target: (evidence[target], self.weights[char, target]))
        return forms[0] if forms else self.characters.get(char, char)

    def longest(self, text, start):
        for end in range(min(len(text), start + self.max_chars), start + 1, -1):
            phrase = text[start:end]
            if phrase in self.phrases:
                return end, phrase
        return start + 1, text[start:start + 1]

    def parts(self, text, reading=()):
        start = 0
        while start < len(text):
            end, phrase = self.longest(text, start)
            # 片面 mustn't swallow 面包 in 一片面包
            for next_start in range(start + 1, end):
                _, other = self.longest(text, next_start)
                if (len(other), self.counts.get(other, 0)) > (len(phrase), self.counts.get(phrase, 0)):
                    end, phrase = start + 1, text[start]
                    break
            if len(phrase) > 1:
                target = self.phrases[phrase]
                subreading = reading[start:end] if len(reading) == len(text) else ()
                forms = self.attested.get((phrase, subreading), self.alternatives.get(phrase, [target]))
                yield list(dict.fromkeys(forms))
            else:
                char_readings = self.readings.get(phrase, {})
                pronunciation = (reading[start],) if len(reading) == len(text) else ()
                forms = char_readings.get(pronunciation, ()) if len(char_readings) > 1 else ()
                yield [self.character(text, start, forms)]
            start = end

    def spellings(self, key):
        forms = self.attested.get(key)
        if forms is None:
            forms = list(dict.fromkeys(''.join(parts) for parts in product(*self.parts(key[0], key[1]))))
        aliases = (''.join(chars) for form in forms
                   for chars in product(*([char, *self.glyphs.get(char, [])] for char in form)))
        return list(dict.fromkeys([*forms, *aliases]))
