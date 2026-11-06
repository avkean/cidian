"""Normalization and checks for the curated usual-word marks."""
import json
from pathlib import Path
import re
import sys
import unicodedata

USAGE = Path(__file__).resolve().parent.parent / 'data' / 'english-usage.tsv'
CONTRACTIONS = {fields[0]: fields[2] for fields in
                (line.split('\t') for line in USAGE.read_text().splitlines() if not line.startswith('#'))
                if len(fields) == 3 and fields[1] == 'C'}


def normalize(text):
    text = text.lower()
    if not text.isascii():  # slow, and ascii has no accents
        text = ''.join(c for c in unicodedata.normalize('NFD', text)
                       if not unicodedata.combining(c)).replace('’', "'")
    if "'" in text:
        text = re.sub(r"[^\W_]+(?:'[^\W_]+)+", lambda m: CONTRACTIONS.get(m[0], m[0]), text)
    text = re.sub(r"(?<=\w)'s\b", '', text)
    return ' '.join(re.findall(r'[^\W_]+', text, re.UNICODE))


def load_marks(path):
    # empty marks are valid finished rows
    marks, queries = {}, set()
    if not path.exists():
        return marks
    for line in path.read_text().splitlines():
        row = json.loads(line)
        en = row['en']
        if not en or en != normalize(en) or en in queries:
            raise ValueError(f'{path}: duplicate or unnormalized query {en!r}')
        queries.add(en)
        if not isinstance(row['marks'], list) or len(row['marks']) > 3:
            raise ValueError(f'{path}: expected zero to three marks for {en}')
        seen = set()
        for order, mark in enumerate(row['marks']):
            key = mark['k']
            if key.count('|') != 1 or key in seen:
                raise ValueError(f'{path}: invalid mark for {en}: {mark}')
            seen.add(key)
            marks.setdefault(key, {})[en] = order
    return marks


def check_dropped(marks, used):
    total = sum(len(queries) for queries in marks.values())
    dropped = sum(len(queries) for key, queries in marks.items() if key not in used)
    for key in sorted(marks.keys() - used):
        print(f'Warning: dropping usual marks for missing entry {key}: '
              f'{", ".join(marks[key])}', file=sys.stderr)
    if dropped > total * 0.01:
        raise ValueError(f'{dropped}/{total} usual marks lost (more than 1%); update the marks before building')
    return total, dropped
