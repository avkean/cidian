"""Read the project's word lists by role."""
from pathlib import Path

PATH = Path(__file__).resolve().parents[1] / 'data/english-stopwords.tsv'


def words(role):
    found = set()
    for number, line in enumerate(PATH.read_text(encoding='utf-8').splitlines(), 1):
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        fields = line.split('\t')
        if len(fields) != 2:
            raise ValueError(f'{PATH}:{number}: expected 2 tab-separated fields')
        word, roles = (field.strip() for field in fields)
        if role in (value.strip() for value in roles.split(',')):
            found.add(word)
    return found
