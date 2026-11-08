"""Mechanical checks for shown meanings, from docs/definitions.md.

usage: definition_rules.py DICTIONARY.jsonl CEDICT.txt.gz   (counts curated senses that break a rule)
"""
import gzip, json, re, sys
from collections import Counter

QUALIFIER = re.compile(r"\b(?:especially|esp\.)", re.I)
EXPLANATION = re.compile(r"^(?:a|an|the|any|one|someone|something|a person|people)\b", re.I)
NOTE = re.compile(r"\s*\([^()]*\)")
LONG = 60  # characters in one shown meaning, notes in brackets left out
# the main word inside a qualifier after an explanation ("a fly, especially a housefly"); the build refuses it
QUALIFIED_MAIN = re.compile(r"^(?:a|an|the|any|someone|something)\b[^;()]*,\s*(?:especially|esp\.)", re.I)


def repeated_label(sense):
    """A round-bracket note that only repeats one of the meaning's own labels, "(classical) to ask" labelled classical."""
    own = {label.lower() for label in sense.get("labels", [])}
    return any(note.strip().lower() in own for note in re.findall(r"\(([^()]*)\)", sense["text"]))


def segments(text):
    return [NOTE.sub("", part).strip().lower() for part in text.split(";") if NOTE.sub("", part).strip()]


def sense_problems(text):
    """Rule ids a shown meaning breaks on its own: 'qualifier', 'explanation', 'long', 'square brackets'."""
    found = []
    if QUALIFIER.search(text):
        found.append("qualifier")
    first = segments(text)[:1]
    if first and EXPLANATION.match(first[0]) and len(first[0].split()) >= 4:
        found.append("explanation")
    if len(NOTE.sub("", text)) > LONG:
        found.append("long")
    if "[" in text or "]" in text:
        found.append("square brackets")
    return found


def hidden_main(shown, cc_first):
    """CC-CEDICT's main equivalent, a word or two, that no shown meaning names at all."""
    head = segments(cc_first)[:1]
    if not head or len(head[0].split()) > 2 or not re.fullmatch(r"(?:to )?[a-z' -]+", head[0]) or head[0].startswith("surname"):
        return None
    phrase = head[0].removeprefix("to ")
    named = any(re.search(rf"\b{re.escape(phrase)}\b", sense.lower()) for sense in shown)
    return None if named else head[0]


def read_cedict(path):
    first = {}
    for line in gzip.open(path, "rt", encoding="utf-8"):
        match = re.match(r"\S+ (\S+) \[([^\]]+)\] /(.*)/$", line.strip())
        if match:
            key = (match[1], re.sub(r"5\b", "", match[2].lower().replace("u:", "ü")).strip())
            first.setdefault(key, match[3].split("/")[0])
    return first


if __name__ == "__main__":
    cedict = read_cedict(sys.argv[2])
    counts, entries, senses = Counter(), Counter(), 0
    for line in open(sys.argv[1], encoding="utf-8"):
        record = json.loads(line)
        core = [s for s in record["senses"] if s.get("source") == "CORE"]
        if not core:
            continue
        entries["core entries"] += 1
        bad = False
        for sense in core:
            senses += 1
            for problem in sense_problems(sense["text"]):
                counts[problem] += 1
                bad = True
        cc = cedict.get((record["simplified"], record["pinyin"].lower()))
        if cc and hidden_main([s["text"] for s in core], cc):
            counts["hidden main (entries)"] += 1
            bad = True
        entries["entries breaking a rule"] += bad
    print(json.dumps({"core senses": senses, **entries, **counts}, indent=1))
