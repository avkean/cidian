"""English source agreement (english-support.tsv), collected during the dictionary build."""
from collections import defaultdict
import gzip
import json
import re

from stage_cache import file_sha256
from usual_words import normalize


def equivalents(text):
    text = re.sub(r'\([^()]*\)|\[[^\[\]]*\]', '', text)
    for part in text.split(';'):
        query = normalize(part)
        if query:
            yield query.removeprefix('to ')


def reading(text):
    return re.sub(r'[05]\b', '', text.lower().replace('u:', 'ü').replace('v', 'ü'))


def mandarin_translations(row):
    # tables sit on entries and on senses
    for section in (row, *row.get('senses', [])):
        for translation in section.get('translations', []):
            if translation.get('lang_code', translation.get('code')) == 'cmn':
                yield from (part.strip() for part in re.split(r'[/,;，；]', translation.get('word', ''))
                            if part.strip())


def english_translations(english):
    pairs = {}
    with gzip.open(english, 'rt', encoding='utf-8') as stream:
        for line in stream:
            row = json.loads(line)
            word = row.get('word', '')
            if not isinstance(word, str) or word != word.lower() or not re.fullmatch(r"[a-z][a-z' -]*", word):
                continue
            query = normalize(word).removeprefix('to ')
            if not query:
                continue
            for form in mandarin_translations(row):
                pairs[query, form] = None
    return list(pairs)


class SupportBuilder:
    # raw, before the core replaces or cleans them
    def __init__(self):
        self.cc = defaultdict(set)
        self.by_form = defaultdict(set)
        self.by_reading = {}
        self.support = defaultdict(dict)

    def add_cedict(self, form, pinyin, glosses):
        queries = self.cc[form, reading(pinyin)]
        for gloss in glosses:
            queries.update(equivalents(gloss))

    def add(self, query, key, bit):
        self.support[query][key] = self.support[query].get(key, 0) | bit

    def add_record(self, word):
        key = word['simplified'] + '|' + word['pinyin']
        for form in [word['simplified'], *word.get('traditional', [])]:
            self.by_form[form].add(key)
            self.by_reading[form, reading(word['pinyin'])] = key
        for sense in word['senses']:
            if sense['source'] == 'CORE':
                for query in equivalents(sense['text']):
                    self.add(query, key, 1)

    def finish(self, dictionary, english, cedict, output, translations):
        for form_reading, queries in self.cc.items():
            key = self.by_reading.get(form_reading)
            if key:
                for query in queries:
                    self.add(query, key, 4)
        self.cc.clear()
        for query, form in translations:
            for key in self.by_form.get(form, ()):
                self.add(query, key, 2)
        temporary = output.with_suffix(output.suffix + '.tmp')
        with temporary.open('w', encoding='utf-8') as stream:
            stream.write('# query\theadword|reading\tbits: core=1, English-Wiktionary=2, CC-CEDICT=4\n')
            for query, entries in sorted(self.support.items()):
                for key, bits in sorted(entries.items()):
                    stream.write(f'{query}\t{key}\t{bits}\n')
        temporary.replace(output)
        manifest = {'inputs': {('data/dictionary.jsonl' if p == dictionary else p.name): file_sha256(p)
                               for p in (dictionary, english, cedict)},
                    'output_sha256': file_sha256(output), 'queries': len(self.support),
                    'pairs': sum(map(len, self.support.values())),
                    'sources': {'core': 'Curated equivalents, CC BY-SA 4.0',
                                'english': 'https://kaikki.org/dictionary/English/kaikki.org-dictionary-English.jsonl.gz (Wiktionary, CC BY-SA 4.0)',
                                'cedict': 'https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz (CC BY-SA 4.0)'}}
        return manifest
