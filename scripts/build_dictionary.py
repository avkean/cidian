"""Build data/dictionary.jsonl and data/english-support.tsv from public sources.

Meanings come from CC-CEDICT, then English Wiktionary (kept only where they add words the
CC-CEDICT meanings lack); examples from Wiktionary and Tatoeba; HSK levels from the complete
HSK vocabulary; ranking scores from jieba and MOT news counts (Chinese Wikipedia counts where
the news has none). --with-core lets the curated core (data/core) replace the imported meanings
and examples of the words it covers, and adds core words the other sources lack. Downloads go
to --cache (default .cache/sources or $CIDIAN_CACHE/sources); Unihan goes to --unihan-cache
(default .cache/unihan or $CIDIAN_CACHE/unihan). --reuse-downloads keeps source copies already there.
"""

import argparse
import ctypes
import json
import os
import re
import shutil
import sys
import tempfile
import unicodedata
import urllib.request
import zipfile
from collections import Counter, defaultdict
from datetime import datetime, timezone
from pathlib import Path

from usual_words import check_dropped, load_marks
from english_support import SupportBuilder, english_translations
import stage_cache
from sources import (
    content_words, download, format_numbered, is_hanzi, load_hsk, load_reading_counts, load_traditional_variants, read_sources,
)


ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data"
CACHE = Path(os.environ.get("CIDIAN_CACHE") or ROOT / ".cache")
HSK_COMMIT = "9837ce887ada0dd593619034fddad850d4b71747"
JIEBA_COMMIT = "67fa2e36e72f69d9134b8a1037b83fbb070b9775"
# name: (url, cache file, sha256, bytes). The hash is checked on every download and every reused file.
# CC-CEDICT, Tatoeba and the Wiktionary dumps change upstream without notice, so the build reads our frozen
# copies in a dated source release. Taking newer ones is a deliberate step.
DATA_RELEASE = "https://git.avkean.com/avkean/cidian/releases/download/sources-2026-11-02"
DOWNLOADS = {
    "english": (f"{DATA_RELEASE}/kaikki-english-trimmed.jsonl.gz", "kaikki-english-trimmed.jsonl.gz",
                "0411d07382c278fda2d0832a23b9692d9c920074096e8b0a94093a57877436d3", 1393356),
    "cedict": (f"{DATA_RELEASE}/cedict.txt.gz", "cedict.txt.gz",
               "31b9443682a1c0da94f8d4d296f58c2f989313ee26e64fb862336e6d3c84c486", 3982717),
    "wiktionary": (f"{DATA_RELEASE}/kaikki-chinese-trimmed.jsonl.gz", "kaikki-chinese-trimmed.jsonl.gz",
                   "6032af80b44a2cb1c8174b5be81228dbbab3ceb8e8a989fc45f7e720b9c1d6f5", 39450196),
    "tatoeba": (f"{DATA_RELEASE}/cmn-eng.zip", "cmn-eng.zip",
                "d606361aa7312dcdfb15ef4fc51cb1e7ed8bc2dc608a7b192626cd36e8403106", 1386340),
    "hsk": (f"https://raw.githubusercontent.com/drkameleon/complete-hsk-vocabulary/{HSK_COMMIT}/complete.json", "hsk.json",
            "c869a0ce353279c9333d9b42c31fc3549785e8b40673dab57ee42bc99cd14131", 9763716),
    "jieba": (f"https://raw.githubusercontent.com/fxsjy/jieba/{JIEBA_COMMIT}/jieba/dict.txt", "jieba.txt",
              "7197c3211ddd98962b036cdf40324d1ea2bfaa12bd028e68faa70111a88e12a8", 5071852),
}
LICENSES = {
    "CC": "CC-CEDICT, Creative Commons Attribution-ShareAlike 4.0, https://cc-cedict.org/",
    "WIKT": "English Wiktionary via kaikki.org (wiktextract), CC BY-SA 4.0 and GFDL, https://kaikki.org/",
    "TATOEBA": "Tatoeba sentences via manythings.org, CC BY 2.0 FR, https://tatoeba.org/",
    "CORE": "Curated dictionary, written for avkean 词典, CC BY-SA 4.0",
    "HSK": "complete-hsk-vocabulary by Yanis Zafirópulos, MIT License",
    "JIEBA": "jieba dictionary word counts, MIT License",
    "UNIHAN": "Unihan 17.0.0 kHanyuPinlu, Unicode License V3, https://www.unicode.org/license.txt",
    "CORPUS": "Multilingual Open Text v1.11 Mandarin news substring counts, CC BY 4.0, https://github.com/bltlab/mot/",
    "WIKIPEDIA": "Chinese Wikipedia substring counts for words missing from the news counts, CC BY-SA 4.0, https://zh.wikipedia.org",
}

MAX_ADDED_SENSES = 6


def fetch(name, cache, reuse):
    url, filename, expected, expected_size = DOWNLOADS[name]
    path = cache / filename
    partial = None
    candidate = path
    try:
        if not (reuse and path.exists()):
            print(f"Downloading {url}")
            request = urllib.request.Request(url, headers={"User-Agent": "cidian-build"})
            try:
                with tempfile.NamedTemporaryFile(dir=cache, prefix=filename + ".", suffix=".part",
                                                 delete=False) as output:
                    partial = candidate = Path(output.name)
                    download(request, output, expected_size)
            except OSError as error:
                raise ValueError(f"{name}: could not download {url} ({error}). Check the URL in DOWNLOADS, or put a "
                                 f"copy with the right hash at {path} and build with --reuse-downloads.") from error
        digest = stage_cache.file_sha256(candidate)
        if digest != expected:
            raise ValueError(f"{name}: {candidate} has sha256 {digest}, expected {expected}. Build again "
                             "without --reuse-downloads to fetch the copy DOWNLOADS names. To take a newer source, "
                             "update DATA_RELEASE and DOWNLOADS together.")
        size = candidate.stat().st_size
        if partial is not None:
            partial.replace(path)
        return path, {"url": url, "sha256": digest, "bytes": size}
    finally:
        if partial is not None:
            partial.unlink(missing_ok=True)


def load_frequency(jieba_path):
    jieba, particles = {}, set()
    for line in jieba_path.open(encoding="utf-8"):
        parts = line.split()
        if len(parts) >= 2 and parts[1].isdigit():
            jieba[parts[0]] = int(parts[1])
            # tags u* and y mark particles such as 了 and 吗
            if len(parts) > 2 and len(parts[0]) == 1 and (parts[2] == "y" or parts[2].startswith("u")):
                particles.add(parts[0])
    return jieba, particles, {"scores": "jieba word counts", "jieba_words": len(jieba)}


def read_counts(name):
    metadata = json.loads((DATA / 'source-inputs.json').read_text())[name.lower()]
    path = ROOT / metadata['counts_file']
    if stage_cache.file_sha256(path) != metadata['counts_sha256']:
        raise ValueError(f'{name} counts hash mismatch; rebuild the counts and source-inputs.json together')
    with path.open(encoding='utf-8') as source:
        counts = {word: int(count) for word, count in
                  (line.rstrip('\n').split('\t') for line in source)}
    if any(count <= 0 for count in counts.values()):
        raise ValueError('corpus counts must be observed positive integers')
    return counts, metadata


def load_corpus_counts():
    """MOT news counts as they are, plus Wikipedia counts (scaled to MOT's size by characters,
    at least 1) for the headwords the news never saw. Never estimate a count for an unobserved word."""
    counts, news = read_counts('Corpus')
    wiki, wikipedia = read_counts('Wikipedia')
    if counts.keys() & wiki.keys():
        raise ValueError('Wikipedia counts must only cover headwords missing from the news counts')
    scale = news['characters'] / wikipedia['characters']
    counts.update((word, max(1, round(n * scale))) for word, n in wiki.items())
    return counts, {'news': news, 'wikipedia': wikipedia}


def reading_shares(readings, core, hsk, unihan):
    # unihan wins where it knows the character; +1 keeps unattested readings positive
    evidence = Counter()
    for headword, reading in list(core) + list(hsk):
        evidence[(headword, reading)] += 1
        if len(headword) > 1 and len(headword) == len(reading):
            for char, syllable in zip(headword, reading):
                evidence[(char, (syllable,))] += 1
    shares = {}
    for headword, keys in readings.items():
        observed = unihan.get(headword, {})
        weights = {key: observed.get(key[1], 0) + 1 if observed
                   else evidence[key] + 1 for key in keys}
        total = sum(weights.values())
        shares.update({key: weight / total for key, weight in weights.items()})
    return shares


def load_tatoeba(path):
    sentences = {}
    with zipfile.ZipFile(path) as archive, archive.open("cmn.txt") as file:
        for line in file.read().decode("utf-8").splitlines():
            parts = line.split("\t")
            if len(parts) < 3:
                continue
            english, chinese, attribution = parts[:3]
            if chinese not in sentences and 4 <= len(chinese) <= 24:
                sentences[chinese] = {"english": english, "attribution": attribution}
    return sentences


def tatoeba_credit(text):
    pairs = re.findall(r"#(\d+) \(([^)]+)\)", text)
    if len(pairs) != 2:
        raise ValueError("Sentence attribution has no pair of authors and source IDs")
    return [{"id": int(number), "author": author} for number, author in pairs]


def shown_written(written, senses):
    """Wiktionary's word breaks as plain spaces. A tagged idiom it splits 2+2 gets the official hyphen between
    the halves. Apostrophes are left to the page, which adds them as it does for any word."""
    breaks = re.sub(" +", " ", re.sub(r"['\u2019-]", lambda match: " " if match[0] == "-" else "", written))
    idiom = any(sense.get("pos") == "idiom" or sense["text"].startswith("(idiom)") for sense in senses)
    halves = breaks.split(" ")
    if idiom and len(halves) == 2 and all(len(re.findall("[1-5]", half)) == 2 for half in halves):
        return "-".join(halves)
    return breaks


# GB/T 16159-2012 6.3.2-6.3.3: a people, language or -ism named after a name the dictionary
# already shows capitalised (蒙古 Měnggǔ, 蒙古人 Měnggǔrén) takes the capital; other lowercase words keep theirs
NAME_SUFFIXES = {"人": "people", "族": "people", "语": "language", "话": "language", "文": "language", "主义": "ism"}


def name_capital(simplified, pinyin, names):
    """(suffix class, pinyin with its first letter capitalised, suffix) or None. names: lowercase numbered pinyin of
    each capitalised multi-syllable headword, by headword"""
    for suffix, kind in NAME_SUFFIXES.items():
        name = simplified[:-len(suffix)]
        if simplified.endswith(suffix) and names.get(name) and pinyin.islower() and pinyin.startswith(names[name] + " "):
            return kind, pinyin[0].upper() + pinyin[1:], suffix
    return None


def name_written(syllables, suffix, split, written=""):
    """The written form with its capital on the name part, or "" when the page's own spacing already does it.
    A long word the page splits into words (5+ characters, suffix a word) would capitalise its second word too,
    so it gets a written form with the suffix apart"""
    if written:
        return written[0].upper() + written[1:]
    if not split:
        return ""
    parts = [syllables[:-len(suffix)], syllables[-len(suffix):]]
    return " ".join("".join(f"{letters}{tone}" for letters, tone in part) for part in parts).capitalize()


def shown_problems(record):
    texts = [sense.get(field) or "" for sense in record["senses"] for field in ("text", "note")]
    texts += [example.get(field) or "" for example in record.get("examples", []) for field in ("chinese", "english")]
    problems = {"link": any(re.search(r"\]\(|https?://|www\.", text) for text in texts)}
    for example in record.get("examples", []):
        # only core pinyin is syllable by syllable; latin letters and digits read unpredictably
        if example["source"] == "CORE" and example.get("pinyin") and not re.search(r"[A-Za-z0-9]", example["chinese"]):
            letters = re.findall(r"[^\W\d_]+", example["pinyin"])
            # erhua joined to its syllable (dianr3) stands for two characters
            count = len(letters) + sum(len(word) > 1 and word.lower().endswith("r") and word.lower() != "er" for word in letters)
            problems["example pinyin"] = problems.get("example pinyin") or count != sum(
                is_hanzi(char) or char == "〇" for char in example["chinese"])
    return [name for name, found in problems.items() if found]


NOTE = re.compile(r"\([^)]*\)|\[[^]]*\]")
ROUND = re.compile(r"\([^)]*\)")


def gloss_parts(text):
    # split on ; or / outside brackets, as search does
    parts, depth, start = [], 0, 0
    for index, char in enumerate(text):
        if char in "([":
            depth += 1
        elif char in ")]":
            depth = max(depth - 1, 0)
        elif char in ";/" and depth == 0:
            parts.append(text[start:index])
            start = index + 1
    return parts + [text[start:]]


def fold(text):
    # roughly how search compares meanings
    text = text.lower()
    if not text.isascii():  # slow, and ascii has no accents
        text = "".join(c for c in unicodedata.normalize("NFKD", text) if not unicodedata.combining(c))
    text = " ".join(text.split())
    for lead in ("to ", "a ", "an ", "the "):
        if text.startswith(lead):
            return text[len(lead):]
    return text


def toneless(text):
    return re.sub(r"[^a-z]", "", unicodedata.normalize("NFKD", text.lower()).replace("v", "u"))


def plain_name(text):
    # "New York, a city" gives "New York"; "surname Tan" and "see 雁山区" give None
    head = re.split(r"[,;(:]", text, maxsplit=1)[0].strip()
    if head[:1].isupper() and not re.search(r"[\u3400-\u9fff0-9\[\](){}]", head):
        return head
    return None


def search_glosses(word, shown):
    # cc-cedict equivalents a curated entry hides, for search only (眼镜 "spectacles", 汽车 "automobile")
    # names give just the plain name; a form that is only the reading ("Ai" for 哀) is left to pinyin
    # no wiktionary: it mixes in other languages' senses (汽车 "train" is japanese)
    seen = set()
    for sense in shown:
        for part in NOTE.sub("", sense["text"]).split(";"):
            seen |= {fold(part), fold(part.split(",")[0])}
    reading = toneless("".join(letters for letters, _ in word["syllables"]))
    terms = []
    for sense in word["senses"]["CC"]:
        head = sense["text"] in word["names"] and plain_name(sense["text"])
        if head:
            candidates = [name.strip() for name in head.split(" or ")]
            candidates = [name for name in candidates if len(name.split()) <= 4]
        else:
            # comma pieces aren't equivalents ("first come, first served"); notes stay, since
            # "sleep (dried mucus...)" isn't plain "sleep"; "Taiwan pr. [pian2]" is about the word
            candidates = []
            for part in gloss_parts(sense["text"]):
                term = re.sub(r"\s+", " ", NOTE.sub("", part)).strip().lower()
                words = re.findall(r"[a-z]+", term)
                if words and len(words) <= 4 and re.fullmatch(r"[a-z' .-]+", term) and "[" not in ROUND.sub("", part):
                    candidates.append(re.sub(r"\s+", " ", part).strip().lower())
        for term in candidates:
            bare = NOTE.sub("", term).strip()
            if toneless(bare) != reading and fold(bare) not in seen:
                seen.add(fold(bare))
                terms.append(term)
    return terms[:12]


MEASURE_WORD_REPEAT = re.compile(r"\b(measure words?)(?:\s*[;,]\s*|\s+or\s+)classifiers?\b|\bclassifiers?(?:\s*[;,]\s*|\s+or\s+)(measure words?)\b", re.I)
CLASSIFIER = re.compile(r"(-)?\bclassifier(s?)\b", re.I)


def one_term(text):
    """Chinese measure words are called classifiers in some sources; shown text says "measure word" only."""
    text = MEASURE_WORD_REPEAT.sub(lambda match: match.group(1) or match.group(2), text)

    def swap(match):
        word = "Measure word" if match.group().lstrip("-")[0].isupper() else "measure word"
        return (" " if match.group(1) else "") + word + match.group(2)
    return CLASSIFIER.sub(swap, text)


def example_terms(examples):
    return [dict(example, english=one_term(example["english"])) if example.get("english") else example for example in examples]


def drop_case_repeats(senses, examples):
    """A meaning repeated with only different capitals ("macaranga", "Macaranga") is shown once, but only
    when its part of speech, labels and note match too, so nothing shown is lost.
    Examples of a dropped meaning move to the one kept."""
    kept, seen, moved = [], {}, {}
    for sense in senses:
        key = (sense["text"].lower(), sense.get("pos"), tuple(sense.get("tags") or ()), sense.get("note"))
        first = seen.get(key)
        if first is None or (sense.get("id") and not first.get("id")):
            seen.setdefault(key, sense)
            kept.append(sense)
        else:
            if sense.get("id"):
                moved[sense["id"]] = first["id"]
    return kept, [dict(example, sense_id=moved[example["sense_id"]]) if example.get("sense_id") in moved else example
                  for example in examples]


def final_senses(word):
    primary_source = next((source for source in ("CC", "WIKT") if word["senses"][source]), None)
    if not primary_source:
        return []
    result = [dict(sense, source=primary_source) for sense in word["senses"][primary_source]]
    covered, added = set(), 0
    texts = {sense["text"].lower() for sense in result}
    for sense in result:
        # a pointer ("see X") explains nothing, so it covers nothing
        if not sense.get("reference_only"):
            covered |= content_words(sense["text"])
    for source in {"CC", "WIKT"} - {primary_source}:
        for sense in word["senses"][source]:
            if sense.get("form_of"):
                continue
            words = content_words(sense["text"])
            if added >= MAX_ADDED_SENSES or sense["text"].lower() in texts:
                continue
            # skip near-repeats and ones that only explain a shown term ("Twitter (a social-networking website)")
            head = content_words(re.sub(r"\((?:[^()]|\([^()]*\))*\)", " ", sense["text"]))
            if (words and len(words & covered) >= 0.6 * len(words)) or (not words and covered) or (head and head <= covered):
                continue
            added += 1
            result.append(dict(sense, source=source))
            covered |= words
            texts.add(sense["text"].lower())
    for sense in result:
        sense.pop("form_of", None)
        if not sense["pos"]:
            sense.pop("pos")
        if not sense["tags"]:
            sense.pop("tags")
    return result


def segment(sentence, lexicon, max_length):
    n = len(sentence)
    best = [None] * (n + 1)
    best[n] = (0, None)
    for start in range(n - 1, -1, -1):
        options = []
        for end in range(start + 1, min(n, start + max_length) + 1):
            piece = sentence[start:end]
            if end == start + 1 or piece in lexicon:
                cost = best[end][0] + 1 + (0 if piece in lexicon else 1)
                options.append((cost, end))
        best[start] = min(options)
    pieces, position = [], 0
    while position < n:
        end = best[position][1]
        pieces.append(sentence[position:end])
        position = end
    return pieces


def swap_data(staged):
    libc = ctypes.CDLL(None, use_errno=True)
    source, target = os.fsencode(staged), os.fsencode(DATA)
    if sys.platform == "darwin":
        rename = libc.renamex_np
        rename.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint)
        arguments = (source, target, 2)  # RENAME_SWAP
    elif sys.platform == "linux":
        rename = libc.renameat2
        rename.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
        arguments = (-100, source, -100, target, 2)  # AT_FDCWD, RENAME_EXCHANGE
    else:
        raise OSError("Atomic data replacement requires Linux or macOS")
    if rename(*arguments):
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error), str(DATA))


def build(args):
    with tempfile.TemporaryDirectory(prefix=".cidian-data-", dir=DATA.parent) as directory:
        output = Path(directory) / "data"
        output.mkdir()
        counts = build_outputs(args, output)

        def preserve(source, target):
            if not Path(target).exists():
                os.link(source, target)
            return target

        shutil.copytree(DATA, output, dirs_exist_ok=True, symlinks=True, copy_function=preserve)
        swap_data(output)
    print(json.dumps(counts, indent=2))


def build_outputs(args, output_dir):
    args.cache.mkdir(parents=True, exist_ok=True)
    stage_cache.folder = args.cache / "stages"
    files, provenance = {}, {}
    for name in DOWNLOADS:
        files[name], provenance[name] = fetch(name, args.cache, args.reuse_downloads)

    print("Reading CC-CEDICT")
    corpus_counts, provenance['corpus'] = load_corpus_counts()
    variants = load_traditional_variants(args.unihan_cache)
    core_file = DATA / "core/entries.jsonl" if args.with_core else None
    parsed = stage_cache.cached("sources", read_sources, files["cedict"], files["wiktionary"], files["english"], core_file, variants, corpus_counts)
    words, wikt_new, core_use = parsed["words"], parsed["wikt_new"], parsed["core_use"]
    core_added, core_merged, core_skipped = parsed["core_added"], parsed["core_merged"], parsed["core_skipped"]
    cc_metadata = parsed["cc_metadata"]
    support = SupportBuilder()
    support.cc = parsed["support_cc"]
    unihan = load_reading_counts(args.unihan_cache)
    inputs = json.loads((DATA / "source-inputs.json").read_text())
    provenance["unihan"] = inputs["unihan"]
    provenance["characters"] = inputs["characters"]
    provenance["english_search"] = inputs["english_search"]
    hsk = load_hsk(files["hsk"], unihan)
    jieba, particles, frequency_info = load_frequency(files["jieba"])
    frequency_info['english_ties'] = 'MOT substring count (Wikipedia, scaled, where MOT has none), then nonregional sense and source attestation'

    print("Scoring readings")
    base = {key: float(jieba.get(key[0], 0)) for key in words}
    readings = defaultdict(list)
    for key in words:
        readings[key[0]].append(key)
    char_weight = defaultdict(float)
    for key, word in words.items():
        if len(key[0]) > 1 and base[key] > 0 and len(key[1]) == len(key[0]):
            for char, syllable in zip(key[0], key[1]):
                char_weight[(char, syllable)] += base[key]
    score = {}
    for headword, keys in readings.items():
        if len(keys) == 1:
            score[keys[0]] = base[keys[0]]
            continue
        weights = {}
        for key in keys:
            word = words[key]
            weight = 1.0 + min(sum(len(senses) for senses in word["senses"].values()), 8)
            if len(headword) == 1:
                weight += char_weight[(headword, key[1][0])] if key[1] else 0
            # readings hsk leaves out are rarer
            if key not in hsk and any(other in hsk for other in keys):
                weight *= 0.25
            weights[key] = weight
        # a particle's neutral reading never shows inside longer words, so put it first
        if headword in particles:
            neutral = [key for key in keys if len(key[1]) == 1 and key[1][0][1] == 5]
            for key in neutral:
                weights[key] = max(weights[key], 2 * max(weights[other] for other in keys if other not in neutral))
        total = sum(weights.values())
        for key in keys:
            score[key] = round(base[key] * weights[key] / total, 2)

    shares = reading_shares(readings, core_use, hsk, unihan)
    frequency_info["reading_counts"] = "kHanyuPinlu, else core/HSK occurrence shares, else even; add-one smoothing"

    print("Adding Tatoeba examples")
    tatoeba = load_tatoeba(files["tatoeba"])
    # skip sentences in traditional characters
    to_simplified = {}
    for (headword, _), word in words.items():
        if len(headword) == 1:
            for traditional in word["traditional"]:
                if traditional != headword:
                    to_simplified.setdefault(traditional, headword)
    char_score = {headword: max(score[key] for key in keys) for headword, keys in readings.items() if len(headword) == 1}

    def traditional_text(sentence):
        return any(char in to_simplified and char_score.get(to_simplified[char], 0) > 5 * char_score.get(char, 0)
                   for char in sentence)

    lexicon = set(readings)
    sentence_index = defaultdict(list)
    for sentence in sorted(tatoeba, key=len):
        if traditional_text(sentence):
            continue
        for piece in set(segment(sentence, lexicon, 8)):
            if piece in readings and len(sentence_index[piece]) < 12:
                sentence_index[piece].append(sentence)

    print("Writing data/dictionary.jsonl")
    # readings that keep their old display (see the file)
    excluded = {line.strip() for line in (DATA / "written-exclusions.txt").read_text(encoding="utf-8").splitlines()
                if line.strip() and not line.startswith("#")}
    headwords = {word["simplified"] for word in words.values()}
    names = {word["simplified"]: format_numbered(word["syllables"]).lower() for word in words.values()
             if len(word["syllables"]) > 1 and word["syllables"][0][0][:1].isupper()
             and len(word["syllables"]) == len(word["simplified"])}
    usual_path = DATA / "core/usual.jsonl"
    usual = load_marks(usual_path) if args.with_core else {}
    usual_used, problems = set(), defaultdict(list)
    output = output_dir / "dictionary.jsonl"
    counts = Counter()
    with output.open("w", encoding="utf-8") as file:
        for key in sorted(words, key=lambda key: (key[0], key[1])):
            word = words[key]
            curated = core_use.get(key)
            if curated:
                senses, examples, sources = curated["senses"], curated["examples"], ["CORE"]
                # replaced sources still count as attestations
                sources += [source for source in word['sources'] if source not in sources]
                classifiers = curated["classifiers"] or word["classifiers"]
                counts["core_added" if key in core_added else "core_merged" if key in core_merged else "core_replaced"] += 1
            else:
                senses, sources, classifiers = final_senses(word), word["sources"], word["classifiers"]
                if not senses:
                    continue
                examples = []
                for example in word["examples"]["WIKT"]:
                    if len(examples) >= 8:
                        break
                    if all(example["chinese"] != old["chinese"] for old in examples):
                        examples.append(dict(example, source="WIKT"))
                if len(examples) < 3 and len(readings[key[0]]) == 1:
                    meaning_words = set().union(*(content_words(sense["text"]) for sense in senses))
                    for sentence in sentence_index.get(key[0], []):
                        if len(examples) >= 3:
                            break
                        if not meaning_words & content_words(tatoeba[sentence]["english"]):
                            continue
                        if all(sentence != old["chinese"] for old in examples):
                            examples.append({"chinese": sentence, "english": tatoeba[sentence]["english"], "source": "TATOEBA", "attribution": tatoeba_credit(tatoeba[sentence]["attribution"])})
                            counts["tatoeba_examples"] += 1
            senses = [{field: one_term(value) if field in ("pos", "text", "note") else value
                       for field, value in sense.items()} for sense in senses]
            examples = example_terms(examples)
            shown = len(senses)
            senses, examples = drop_case_repeats(senses, examples)
            counts["case_repeats_dropped"] += shown - len(senses)
            record = {
                "simplified": word["simplified"],
                "traditional": word["traditional"],
                "pinyin": format_numbered(word["syllables"]),
                "senses": senses,
            }
            if word.get("written") and f"{word['simplified']}|{format_numbered(word['syllables'])}" not in excluded:
                record["written"] = shown_written(word["written"], senses)
            if examples:
                record["examples"] = examples
            if classifiers:
                record["classifiers"] = classifiers
            if key in hsk:
                record["hsk"] = hsk[key]
            if score[key] > 0:
                record["frequency"] = score[key]
            if corpus_counts.get(word['simplified'], 0) > 0:
                record['corpus_frequency'] = corpus_counts[word['simplified']]
            record["reading_frequency"] = max(base[key] * shares[key], 0.01)
            record["reading_corpus_frequency"] = corpus_counts.get(word["simplified"], 0) * shares[key]
            record["sources"] = sources
            if word.get("wiktionary_pages") and any(item.get("source") == "WIKT" for item in senses + examples):
                record["wiktionary_pages"] = word["wiktionary_pages"]
            usual_key = record['simplified'] + '|' + record['pinyin']
            if usual_key in usual:
                record['usual_for'] = usual[usual_key]
                usual_used.add(usual_key)
            extra = search_glosses(word, senses) if curated else []
            if extra:
                record["search_glosses"] = extra
                counts["words_with_search_glosses"] += 1
            for problem in shown_problems(record):
                problems[problem].append(usual_key)
            named = name_capital(record["simplified"], record["pinyin"], names)
            if named:
                record["pinyin"] = named[1]
                written = name_written(word["syllables"], named[2], len(record["simplified"]) >= 5
                                       and len(named[2]) > 1 and named[2] in headwords, record.get("written", ""))
                if written:
                    record["written"] = written
            support.add_record(record)
            file.write(json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n")
            counts["words"] += 1
            counts["examples"] += len(examples)
            for source in sources:
                counts[f"words_with_{source}"] += 1
            if any(sense["source"] != senses[0]["source"] for sense in senses):
                counts["words_with_added_meanings"] += 1
    if problems:
        raise ValueError("Fix these in the data: " + "; ".join(
            f"{name} in {len(keys)} records ({', '.join(keys[:10])})" for name, keys in problems.items()))
    counts['usual_marks'], counts['usual_dropped'] = check_dropped(usual, usual_used)
    print("Writing data/english-support.tsv")
    support_manifest = support.finish(output, files["english"], files["cedict"], output_dir / "english-support.tsv",
                   stage_cache.cached("english", english_translations, files["english"]))
    if args.with_core and usual_path.exists():
        provenance['usual'] = {'file': 'data/core/usual.jsonl',
                               'sha256': stage_cache.file_sha256(usual_path)}
    if args.with_core:
        counts["core_skipped"] = core_skipped

    provenance["cedict"]["metadata"] = cc_metadata
    if args.with_core:
        core_folder = DATA / "core"
        provenance["core"] = {
            "file": "data/core/entries.jsonl",
            "sha256": stage_cache.file_sha256(core_folder / "entries.jsonl"),
            "entries": parsed["core_entries"],
            "traditional_from": ["CC", "WIKT", "UNIHAN"],
            "traditional_method": "Public attested spellings, longest phrase match, then public character mappings",
        }
    (output_dir / "dictionary-source.json").write_text(json.dumps({
        "built": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "sources": provenance,
        "english_support": support_manifest,
        "licenses": {key: value for key, value in LICENSES.items() if args.with_core or key != "CORE"},
        "counts": dict(counts, wiktionary_new_words=wikt_new),
        "frequency": frequency_info,
        "method": __doc__.strip(),
    }, ensure_ascii=False, indent=2) + "\n")
    return dict(counts, wiktionary_new_words=wikt_new)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cache", type=Path, default=CACHE / "sources",
                        help="Folder for downloaded source files and cached stages")
    parser.add_argument("--unihan-cache", type=Path, default=CACHE / "unihan",
                        help="Folder for the pinned Unihan download")
    parser.add_argument("--reuse-downloads", action="store_true",
                        help="Use hash-checked files already in the cache instead of downloading the pinned copies")
    parser.add_argument("--with-core", action="store_true",
                        help="Replace imported meanings and examples with the curated core in data/core")
    build(parser.parse_args())


if __name__ == "__main__":
    main()
