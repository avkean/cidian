"""Parse and merge the sources: the stages build_dictionary.py caches."""

import gzip
import json
import re
import tempfile
import time
import unicodedata
import urllib.request
import zipfile
from collections import Counter, defaultdict
from pathlib import Path

from english_support import SupportBuilder
from stage_cache import cached, file_sha256
from stopwords import words
from traditional import Traditional


EMPTY_MEANING = re.compile(
    r"(only )?used in |the meaning of this|(an? )?(alternative|variant|nonstandard|obsolete|archaic|suspected)\b|see ", re.I)
WIKT_POS = {
    "noun": "noun", "verb": "verb", "adj": "adjective", "adv": "adverb", "name": "proper noun",
    "phrase": "phrase", "proverb": "idiom", "pron": "pronoun", "intj": "interjection",
    "conj": "conjunction", "classifier": "measure word", "num": "numeral", "prep": "preposition",
    "particle": "particle", "det": "determiner", "suffix": "suffix", "prefix": "prefix",
    "postp": "particle", "circumpos": "preposition",
}
WIKT_TAGS = {
    "literary": "literary", "colloquial": "colloquial", "dialectal": "dialect", "archaic": "archaic",
    "obsolete": "obsolete", "figuratively": "figurative", "slang": "slang", "Internet": "internet",
    "Taiwan": "Taiwan", "Mainland-China": "mainland", "derogatory": "pejorative", "humorous": "humorous",
    "honorific": "honorific", "formal": "formal", "informal": "informal", "neologism": "neologism",
    "abbreviation": "abbreviation", "euphemistic": "euphemistic", "vulgar": "vulgar",
    "Classical-Chinese": "classical", "rare": "rare", "polite": "polite", "dated": "dated",
}
TOPOLECTS = {
    "Cantonese", "Hokkien", "Teochew", "Wu", "Hakka", "Min", "Min-Nan", "Min-Dong", "Min-Bei",
    "Gan", "Xiang", "Jin", "Shanghainese", "Taishanese", "Hainanese", "Puxian-Min",
    "Leizhou-Min", "Dungan", "Sichuanese", "Hong-Kong", "Malaysia", "Singapore", "Northern-Min",
    "Southern-Min", "Eastern-Min", "Suzhounese", "Ningbonese", "Wenzhounese", "Pinghua",
    "Philippine-Hokkien", "Taiwanese-Hokkien", "Zhangzhou-Hokkien", "Xiamen", "Quanzhou",
}
# 遮 lists 这 as an alternative simplification
NOT_STANDARD_FORM = {"Second-Round-Simplified-Chinese", "alternative", "nonstandard"} | TOPOLECTS
NONSTANDARD_SOUND = {"colloquial", "dialectal", "contraction", "phonetic", "Erhua", "nonstandard", "Beijing"}
SENSE_SKIP = words('sense')
TONE_MARKS = {"̄": 1, "́": 2, "̌": 3, "̀": 4}


def is_hanzi(char):
    code = ord(char)
    return 0x3400 <= code <= 0x9FFF or 0xF900 <= code <= 0xFAFF or 0x20000 <= code <= 0x323AF


def numbered_syllables(text):
    syllables = []
    for token in re.split(r"[\s·,，'’]+", text.replace("u:", "ü").replace("U:", "Ü")):
        match = re.fullmatch(r"([A-Za-zÜüVv]+)([0-5]?)", token)
        if not match:
            continue
        letters = match.group(1).replace("v", "ü").replace("V", "Ü")
        syllables.append((letters, int(match.group(2) or 5) or 5))
    return syllables


def key_of(syllables):
    return tuple((letters.lower(), tone) for letters, tone in syllables)


def format_numbered(syllables):
    return " ".join(f"{letters}{tone}" if tone < 5 else letters for letters, tone in syllables)


def marked_parse(text, count, valid):
    """The syllables of marked pinyin, and where it was broken (letter offset to space, hyphen or apostrophe)."""
    letters, tones, breaks = [], {}, {}
    for char in unicodedata.normalize("NFD", text.strip()):
        if char in TONE_MARKS:
            if letters:
                tones[len(letters) - 1] = TONE_MARKS[char]
        elif char == "̈":
            if letters and letters[-1] in "uU":
                letters[-1] = "ü" if letters[-1] == "u" else "Ü"
        elif char.isascii() and char.isalpha():
            letters.append(char)
        elif char in " '’-":
            breaks.setdefault(len(letters), "'" if char in "'’" else char)
        else:
            return None, None
    plain = "".join(letters)
    lower = plain.lower()
    best = {0: []}
    for end in range(1, len(plain) + 1):
        for start in range(max(0, end - 6), end):
            if start not in best or len(best[start]) >= count:
                continue
            if any(start < position < end for position in breaks):
                continue
            if lower[start:end] in valid:
                path = best[start] + [(start, end)]
                if end not in best or len(path) < len(best[end]):
                    best[end] = path
    path = best.get(len(plain))
    if not path or len(path) != count:
        return None, None
    result = []
    for start, end in path:
        tone = next((tones[i] for i in range(start, end) if i in tones), 5)
        result.append((plain[start:end], tone))
    return result, breaks


def written_pinyin(text, count, valid):
    """Pinyin as an editor wrote it, with tone numbers: kai1 wan2xiao4, ren2shan1-ren2hai3, Xi1'an1.

    Every syllable carries its tone digit (5 for neutral), so the word grouping survives either tone mode."""
    syllables, breaks = marked_parse(text, count, valid)
    if not syllables:
        return None, None
    written, at = "", 0
    for letters, tone in syllables:
        if at:
            written += breaks.get(at, "")
        written += f"{letters}{tone}"
        at += len(letters)
    return key_of(syllables), written


def content_words(text):
    text = re.sub(r"\[[^\]]*\]", " ", text.lower())
    words = set()
    for word in re.findall(r"[a-z]+(?:'[a-z]+)?|[㐀-鿿]", text):
        if word in SENSE_SKIP:
            continue
        if len(word) > 5:
            word = word.replace("our", "or")  # behaviour, behavior
        if len(word) > 4 and word.endswith("ies"):
            word = word[:-3] + "y"
        elif len(word) > 3 and word.endswith("s") and not word.endswith("ss"):
            word = word[:-1]
        words.add(word)
    return words


CC_LABELS = {
    "bound form": "bound form", "literary": "literary", "coll.": "colloquial", "colloquial": "colloquial",
    "dialect": "dialect", "old": "old", "Tw": "Taiwan", "TW": "Taiwan", "slang": "slang",
    "Internet slang": "internet", "loanword": "loanword", "honorific": "honorific", "polite": "polite",
    "archaic": "archaic", "classical": "classical", "derog.": "pejorative", "pejorative": "pejorative",
    "humorous": "humorous", "euphemism": "euphemistic", "fig.": "figurative", "abbr.": "abbreviation",
    "math.": "mathematics", "chemistry": "chemistry", "physics": "physics", "medicine": "medicine",
    "computing": "computing", "linguistics": "linguistics", "Buddhism": "Buddhism", "TCM": "Chinese medicine",
    "biology": "biology", "botany": "botany", "zoology": "zoology", "law": "law", "music": "music",
    "sports": "sports", "military": "military", "finance": "finance", "economics": "economics",
    "geology": "geology", "astronomy": "astronomy", "anatomy": "anatomy", "Cantonese": "Cantonese",
    "onom.": "onomatopoeia", "of a person": "of a person", "used in names": "used in names",
}


def clean_cc_text(text):
    # 漢字|汉字[han4 zi4] -> 汉字[han4 zi4]
    text = re.sub(r"[^\s\[\]|/]+\|([^\s\[\]|/]+)\[", r"\1[", text)
    # Separate a word's numbered pinyin from a following parenthetical note.
    text = re.sub(rf"({HAN}\[[A-Za-zÜü:]+[0-5](?: [A-Za-zÜü:]+[0-5])*\])\(", r"\1 (", text)
    return re.sub(r"\s+", " ", text).strip()


# a cc sense that only points at other words: "see 黄鼠狼给鸡拜年[huang2 …]", "variant of 逼格[bi1 ge2]". each word is
# "trad|simp[pinyin]", "simp[pinyin]" or bare; the pinyin goes (the linked word shows its own) and the sense is
# flagged reference_only, so search doesn't read "see" or "variant" as its English
CC_REFERENCE = re.compile(
    r"(?P<label>(?:\([^()]{1,30}\) )?(?:see also|see|variant of|old variant of|erhua variant of|archaic variant of|used in|abbr\. for|also written|same as)) (?P<words>.+)")
CC_REFERENCE_WORD = re.compile(r"(?:(?P<trad>[^\s\[\]|/,;]+)\|)?(?P<simp>[^\s\[\]|/,;]+)(?P<pinyin>\[[^\[\]]+\])?")


def cc_reference(text):
    """(readable text, referenced words) when the whole sense is a cross-reference, else None."""
    match = CC_REFERENCE.fullmatch(text)
    if not match:
        return None
    refs, readable = [], []
    words, at = match["words"], 0
    while True:
        part = CC_REFERENCE_WORD.match(words, at)
        if not part or not (part["pinyin"] or re.search(HAN, part[0])):
            return None
        refs.append({"simplified": part["simp"], "traditional": part["trad"] or part["simp"]})
        readable.append(part["simp"])
        at = part.end()
        if at == len(words):
            return f"{match['label']} {' and '.join(readable)}", refs
        separator = re.match(r" and |, ", words[at:])
        if not separator:
            return None
        at += separator.end()


def cc_sense(text):
    text = re.sub(r"\s+", " ", text).strip()
    pos, tags = None, []
    match = re.match(r"^\(([^()]{1,30})\)\s+", text)
    if match:
        labels = [part.strip() for part in match.group(1).split(",")]
        if all(label in CC_LABELS for label in labels):
            tags = [CC_LABELS[label] for label in labels]
            text = text[match.end():]
    reference = cc_reference(text)
    text = reference[0] if reference else clean_cc_text(text)
    if "onomatopoeia" in tags:
        pos = "onomatopoeia"
        tags.remove("onomatopoeia")
    return pos, tags, text, reference[1] if reference else None


def load_cedict(path):
    words, metadata, support = [], {}, SupportBuilder()
    with gzip.open(path, "rt", encoding="utf-8-sig") as file:
        for number, line in enumerate(file, 1):
            line = line.rstrip("\n")
            if line.startswith("#! ") and "=" in line:
                key, value = line[3:].split("=", 1)
                metadata[key] = value
            if not line or line.startswith("#"):
                continue
            match = re.fullmatch(r"(\S+) (\S+) \[(.+?)\] /(.+)/", line)
            if not match:
                raise ValueError(f"Invalid CC-CEDICT line {number}")
            traditional, simplified, pinyin, definitions = match.groups()
            # □ stands in for a character with no unicode form, so it can't be a headword
            if "□" in traditional + simplified:
                continue
            support.add_cedict(simplified, pinyin, definitions.split("/"))
            syllables = numbered_syllables(pinyin)
            if not syllables:
                continue
            senses, classifiers = [], []
            for definition in definitions.split("/"):
                if definition.startswith("CL:"):
                    for item in definition[3:].split(","):
                        simple = item.split("[")[0].split("|")[-1].strip()
                        if simple and simple not in classifiers:
                            classifiers.append(simple)
                    continue
                pos, tags, text, refs = cc_sense(definition)
                if refs:
                    senses.append({"pos": pos, "tags": tags, "text": text, "refs": refs, "reference_only": True})
                elif text:
                    senses.append({"pos": pos, "tags": tags, "text": text})
            variant_notes = [re.fullmatch(r"(?:old |archaic |ancient )?variant of ([^|\[\]\s]+)(?:\|[^|\[\]\s]+)?(?:\[[^\[\]]+\])?", text) for text in definitions.split("/")]
            words.append({
                "source": "CC", "simplified": simplified, "traditional": traditional,
                "syllables": syllables, "senses": senses, "examples": [],
                "glyph_variants": [match[1] for match in variant_notes] if all(variant_notes) else [],
                "classifiers": classifiers,
                "order": (pinyin[:1].isupper(), any("variant of" in sense["text"] for sense in senses), -len(senses)),
            })
    # lines sharing a headword and reading: words before names (水 "water" before "surname Shui"),
    # then without variant notes, then longer (當 "to be" before 噹 "ding dong")
    words.sort(key=lambda word: word.pop("order"))
    return words, metadata, support.cc


# wiktionary writes a referenced word as "traditional／simplified (pinyin, “gloss”)"; the slash, the pinyin
# and the gloss are each optional, but a lone word needs its brackets to count. the pinyin (or other
# romanisation) is dropped since the linked word shows its own; the gloss stays after the simplified word
HAN = r"[\u2e80-\u9fff\uf900-\ufaff\U00020000-\U0003ffff]"
WORD = rf"(?=[A-Za-z0-9]*{HAN})(?:{HAN}|[A-Za-z0-9])+"
WIKT_REFERENCE = re.compile(
    rf"(?<![A-Za-z0-9])(?P<first>{WORD})(?:(?:／| /)(?P<second>{WORD}))?"
    r"(?P<brackets> \((?:(?P<romanised>[^()“”]+?)(?:, (?=“)|(?=\))))?(?:“(?P<gloss>[^”]*)”)?\))?")
# letters with tone marks, tone digits or superscript tones
ROMANISED = re.compile(r"(?=.*[^\x00-\x7f0-9]|.*[0-9])[\w\u0300-\u036f'’ ,\-⁻]+")


def wiktionary_references(text):
    """The text with references cleaned up, and the referenced words as simplified and traditional."""
    refs = []

    def clean(match):
        first, second, brackets, romanised, gloss = match.group("first", "second", "brackets", "romanised", "gloss")
        if second is None and brackets is None or second is not None and len(first) != len(second):
            return match.group(0)
        if romanised and not ROMANISED.fullmatch(romanised) or brackets == " ()":
            # brackets of something else (plain English) stay as written, but a slashed pair is still a pair
            if second is None:
                return match.group(0)
            refs.append({"simplified": second, "traditional": first})
            return second + brackets
        refs.append({"simplified": second or first, "traditional": first})
        return (second or first) + (f" “{gloss}”" if gloss else "")

    return WIKT_REFERENCE.sub(clean, text), refs


def english_abbreviations(path):
    """English Wiktionary's headwords that end in a period ("etc.", "e.g."); a plain word never does."""
    found = set()
    with gzip.open(path, "rt", encoding="utf-8") as file:
        for line in file:
            word = json.loads(line).get("word")
            if isinstance(word, str) and word.endswith(".") and " " not in word:
                found.add(word)
    return frozenset(found)


# some glosses join a description and its short gloss with ".:" ("... “Singapore”.: Singapore")
JOINED = re.compile(r"(\S*)\.(?=:)")


def unjoined(text, abbreviations):
    """The text without a period right before a colon, unless it ends an abbreviation ("etc.:")."""
    def drop(match):
        word = match.group(1).lstrip("([{“‘\"'") + "."
        keep = word in abbreviations or word.endswith("..")
        return match.group(0) if keep else match.group(1)

    return JOINED.sub(drop, text)


def wiktionary_lines(entry):
    # the summary line mixes in colloquial, dialect and taiwan-only readings (我們 wǎnme, 今天 jīnyān),
    # so the detailed standard chinese lines win when there are any
    lines = [(sound["zh_pron"], set(sound.get("tags", []))) for sound in entry.get("sounds", [])
             if "zh_pron" in sound and {"Mandarin", "Pinyin"} <= set(sound.get("tags", []))]
    detailed = [(text, tags) for text, tags in lines if "Standard-Chinese" in tags]
    if detailed:
        chosen = [text for text, tags in detailed
                  if not tags & NONSTANDARD_SOUND and ("Mainland-China" in tags or "Taiwan" not in tags)]
    else:
        chosen = [text for text, tags in lines if tags <= {"Mandarin", "Pinyin", "Standard"}]
    return chosen


def wiktionary_readings(entry, valid):
    readings = []
    hanzi = [char for char in entry["word"] if is_hanzi(char)]
    if len(hanzi) != len(entry["word"]):
        return readings
    for text in wiktionary_lines(entry):
        text = text.split(" (")[0].split("／")[0].strip()
        syllables = marked_parse(text, len(hanzi), valid)[0]
        if syllables and key_of(syllables) not in [key_of(item) for item in readings]:
            readings.append(syllables)
    return readings


def wiktionary_written(entry, valid):
    """Each standard line as written, by its letters and tones; a note after "[" is not part of it."""
    written = {}
    if not all(is_hanzi(char) for char in entry["word"]):
        return written
    for text in wiktionary_lines(entry):
        text = text.split(" [")[0].split(" (")[0].split("／")[0].strip()
        key, form = written_pinyin(text, len(entry["word"]), valid)
        if key:
            written.setdefault(key, form)
    return written


def wiktionary_examples(sense):
    examples = []
    for example in sense.get("examples", []):
        tags = set(example.get("tags", []))
        english = (example.get("english") or example.get("translation") or "").strip()
        if example.get("type") not in (None, "example") or "Traditional-Chinese" in tags:
            continue
        if tags & TOPOLECTS or "Classical-Chinese" in tags or "Literary-Chinese" in tags:
            continue
        text = (example.get("text") or "").strip()
        if not text or not english or len(text) > 40 or not any(is_hanzi(char) for char in text):
            continue
        examples.append({"chinese": text, "pinyin": (example.get("roman") or "").strip(), "english": english})
    return examples


def wiktionary_simplified(entry, forms):
    # fonts can't draw rare simplified forms outside the bmp (𢧐 for 戰)
    if not any(ord(char) > 0xFFFF for char in entry["word"]):
        usable = [form for form in forms if not any(ord(char) > 0xFFFF for char in form)]
        if forms and not usable:
            return None
        forms = usable
    simplified = forms[0] if forms else entry["word"]
    return simplified if len(simplified) == len(entry["word"]) else None


WRITTEN_MIN = 3


WHOLE_SYLLABLE = re.compile(r"[^aeiouü]*[aeiouü].*|m|n|ng|r|hm|hng", re.I)


def attach_written(words, entries, cc_names):
    seen = set()
    for simplified, written, verb_object in entries:
        for syllables, form in written.items():
            key = (simplified, syllables)
            word = words.get(key)
            if not word or key in seen:
                continue
            # the first line to match our letters and tones wins, whether or not it is then used
            seen.add(key)
            count = len(syllables)
            aabb = count == 4 and len(simplified) == 4 and simplified[0] == simplified[1] and simplified[2] == simplified[3]
            if (count >= WRITTEN_MIN and (count == 4 or verb_object) and not aabb
                    and key not in cc_names and not any(char.isupper() for char in form)
                    and not any(char.isupper() for letters, _ in word["syllables"] for char in letters)
                    and all(WHOLE_SYLLABLE.fullmatch(letters) for letters, _ in word["syllables"])):
                word["written"] = form


def load_wiktionary(path, valid, abbreviations):
    """The words, and every entry's standard lines as written with its simplified headword and verb-object tag."""
    words, entries = [], []
    with gzip.open(path, "rt", encoding="utf-8") as file:
        for line in file:
            entry = json.loads(line)
            if entry.get("lang_code") != "zh" or entry.get("pos") not in WIKT_POS and entry.get("pos") != "character":
                continue
            readings = wiktionary_readings(entry, valid)
            written = wiktionary_written(entry, valid)
            if not readings and not written:
                continue
            forms = [form["form"] for form in entry.get("forms", [])
                     if "Simplified-Chinese" in form.get("tags", []) and not NOT_STANDARD_FORM & set(form["tags"])]
            simplified = wiktionary_simplified(entry, forms)
            if simplified is None:
                continue
            if written:
                entries.append((simplified, written, any("verb-object" in sense.get("tags", [])
                                                         for sense in entry.get("senses", []))))
            if not readings:
                continue
            senses, examples = [], []
            for sense in entry.get("senses", []):
                tags = set(sense.get("tags", []))
                glosses = sense.get("glosses") or []
                if not glosses or "no-gloss" in tags:
                    continue
                if tags & TOPOLECTS and "Mandarin" not in tags:
                    continue
                text = glosses[-1].strip()
                text = re.sub(r"\s*\(Classifier:[^)]*\)", "", text).strip()
                text = unjoined(text, abbreviations)
                if not text:
                    continue
                labels = [WIKT_TAGS[tag] for tag in sense.get("tags", []) if tag in WIKT_TAGS]
                form_of = bool(tags & {"alt-of", "form-of"})
                text, refs = wiktionary_references(text)
                senses.append({"pos": WIKT_POS.get(entry["pos"]), "tags": labels, "text": text, "form_of": form_of,
                               **({"refs": refs} if refs else {})})
                examples.extend(wiktionary_examples(sense))
            if senses:
                words.append({
                    "source": "WIKT", "simplified": simplified, "traditional": entry["word"],
                    "traditional_forms": [
                        form["form"] for form in entry.get("forms", [])
                        if "Traditional-Chinese" in form.get("tags", [])
                        and not (NOT_STANDARD_FORM - {"alternative"}) & set(form["tags"])],
                    "readings": readings, "senses": senses, "examples": examples,
                    "classifiers": [],
                })
    return words, entries


def load_hsk(path, unihan=None):
    # the list copies every cc-cedict reading into its forms (的 also gets di1 "taxi"), so with
    # unihan counts a level only sticks to readings at least a tenth as common as the top one
    levels = {}
    for entry in json.loads(path.read_text(encoding="utf-8")):
        found = [label for label in entry.get("level", []) if label.startswith("newest-")]
        found = found or [label for label in entry.get("level", []) if label.startswith("new-")]
        if not found:
            continue
        level = min(int(label.split("-")[1]) for label in found)
        readings = {key_of(numbered_syllables(form.get("transcriptions", {}).get("numeric", "")))
                    for form in entry.get("forms", [])} - {()}
        counts = (unihan or {}).get(entry["simplified"], {})
        top = max((counts.get(reading, 0) for reading in readings), default=0)
        if top:
            readings = {reading for reading in readings if counts.get(reading, 0) * 10 >= top}
        for reading in readings:
            levels[(entry["simplified"], reading)] = min(level, levels.get((entry["simplified"], reading), 9))
    return levels


UNIHAN_VERSION = "17.0.0"
UNIHAN_SHA256 = "f7a48b2b545acfaa77b2d607ae28747404ce02baefee16396c5d2d7a8ef34b5e"
UNIHAN_BYTES = 8518517
UNIHAN_URL = f"https://www.unicode.org/Public/{UNIHAN_VERSION}/ucd/Unihan.zip"


def download(request, output, expected_size):
    deadline = time.monotonic() + 600
    received = 0
    with urllib.request.urlopen(request, timeout=120) as response:
        while True:
            if time.monotonic() >= deadline:
                raise TimeoutError("Download deadline exceeded")
            # read1 returns after one read, so a trickle cannot keep a chunk open forever.
            chunk = response.read1(min(1 << 20, expected_size - received + 1))
            if time.monotonic() >= deadline:
                raise TimeoutError("Download deadline exceeded")
            if not chunk:
                break
            received += len(chunk)
            if received > expected_size:
                raise ValueError(f"Download exceeds expected size of {expected_size} bytes")
            output.write(chunk)


def unihan_archive(folder):
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"Unihan-{UNIHAN_VERSION}.zip"
    partial = None
    candidate = path
    try:
        if not path.exists():
            print(f"Downloading {UNIHAN_URL}")
            request = urllib.request.Request(UNIHAN_URL, headers={"User-Agent": "cidian-build"})
            with tempfile.NamedTemporaryFile(dir=folder, prefix=path.name + ".", suffix=".part",
                                             delete=False) as output:
                partial = candidate = Path(output.name)
                download(request, output, UNIHAN_BYTES)
        if file_sha256(candidate) != UNIHAN_SHA256:
            raise ValueError("Unihan archive hash mismatch")
        with zipfile.ZipFile(candidate) as archive:
            if archive.testzip() is not None:
                raise ValueError("Unihan archive CRC mismatch")
        if partial is not None:
            partial.replace(path)
    finally:
        if partial is not None:
            partial.unlink(missing_ok=True)
    return path


def load_traditional_variants(folder):
    with zipfile.ZipFile(unihan_archive(folder)) as archive:
        text = archive.read("Unihan_Variants.txt").decode("utf-8")
    variants = {}
    for line in text.splitlines():
        if line.startswith('#') or not line.strip():
            continue
        code, field, value = line.split('\t')
        if field == 'kTraditionalVariant':
            variants[chr(int(code[2:], 16))] = [chr(int(part[2:], 16)) for part in value.split()]
    return variants


def load_reading_counts(folder):
    counts = defaultdict(dict)
    with zipfile.ZipFile(unihan_archive(folder)) as archive:
        text = archive.read("Unihan_Readings.txt").decode("utf-8")
    if f"# Unicode Version {UNIHAN_VERSION}" not in text:
        raise ValueError("Unihan version mismatch")
    for line in text.splitlines():
        if line.startswith("#") or not line.strip():
            continue
        code, field, value = line.split("\t")
        if field == "kHanyuPinlu":
            for pinyin, count in re.findall(r"([^ ()]+)\((\d+)\)", value):
                unmarked = "".join(c for c in unicodedata.normalize("NFD", pinyin)
                                   if c not in TONE_MARKS)
                plain = unicodedata.normalize("NFC", unmarked).lower()
                reading = key_of(marked_parse(pinyin, 1, {plain})[0] or [])
                if len(reading) != 1 or int(count) <= 0:
                    raise ValueError(f"Invalid kHanyuPinlu: {line}")
                counts[chr(int(code[2:], 16))][reading] = int(count)
    if len(counts) < 3000:
        raise ValueError("Incomplete kHanyuPinlu data")
    return counts


def lowercase_starters(items):
    lower, upper = Counter(), Counter()
    for item in items:
        for sense in item["senses"]:
            for part in sense["text"].split("; "):
                match = re.match(r"[A-Za-z][A-Za-z'’-]*", part)
                if match:
                    (upper if match.group()[0].isupper() else lower)[match.group().lower()] += 1
    return {word for word, count in lower.items() if count >= 3 and count >= 4 * upper[word]}


def plain_gloss(text, starters, proper=False):
    # match the other sources: no final full stop, lowercase start; names like "Mount Tai",
    # "White Lotus" and "The West" stay, since a capitalised word follows
    if re.search(r"(?<!e\.g)(?<!i\.e)\.\s+[A-Z“\"]", text):
        return text
    if text.endswith(".") and not text.endswith(("..", "etc.")) and "." not in text.split()[-1][:-1]:
        text = text[:-1]
    if proper:
        return text

    def lower_start(match):
        label, word, after = match.groups()
        if word.lower() not in starters or (after and word not in ("To", "A", "An")):
            return match.group(0)
        return label + word[0].lower() + word[1:] + (after or "")

    # the lookahead keeps "ASEAN" and "Tarō" whole
    parts = [re.sub(r"^((?:\([^()]*\)\s+)?)([A-Z][a-z'’-]*)(?!\w)(\s+[A-Z])?", lower_start, part)
             for part in text.split("; ")]
    return "; ".join(parts)


FULL_WIDTH = {",": "，", "?": "？", "!": "！", ":": "：", ";": "；"}
CJK = r"[\u3000-\u303f\u3400-\u9fff\uf900-\ufaff\uff00-\uffef“”]"


def full_width(chinese):
    return re.sub(rf"(?<={CJK})\s*([,?!:;])\s*|\s*([,?!:;])\s*(?={CJK})",
                  lambda match: FULL_WIDTH[match.group(1) or match.group(2)], chinese)


def read_core_entries(folder):
    entries = {}
    path = folder / "entries.jsonl"
    if not path.exists():
        return entries
    with path.open(encoding="utf-8") as stream:
        for number, line in enumerate(stream, 1):
            try:
                entry = json.loads(line)
                if not isinstance(entry, dict) or not isinstance(entry.get("key"), str):
                    raise ValueError("expected an object with a string key")
                if entry["key"].count("|") != 1 or not all(entry["key"].split("|")):
                    raise ValueError("expected headword|pinyin key")
                if not isinstance(entry.get("senses"), list):
                    raise ValueError("expected a senses list")
                if entry["key"] in entries:
                    raise ValueError(f"duplicate key {entry['key']!r}")
                entries[entry["key"]] = entry
            except ValueError as error:
                raise ValueError(f"{path}:{number}: invalid core entry: {error}") from error
    return entries


def load_core(folder, starters, traditional):
    entries = read_core_entries(folder)
    # docs/definitions.md: checked on every curated entry, published or not
    from definition_rules import QUALIFIED_MAIN, repeated_label
    broken = [entry["key"] for entry in entries.values() for sense in entry["senses"]
              if QUALIFIED_MAIN.search(sense["text"]) or "[" in sense["text"] or repeated_label(sense)]
    if broken:
        raise ValueError("curated meanings with the main word in a qualifier, square brackets or a label repeated in the text: "
                         + ", ".join(broken[:10]))
    core = {}
    for entry in entries.values():
        simplified, pinyin = entry["key"].split("|")
        syllables = numbered_syllables(pinyin)
        senses = []
        for sense in entry["senses"]:
            item = {}
            if sense.get("pos"):
                item["pos"] = sense["pos"]
            if sense.get("labels"):
                item["tags"] = sense["labels"]
            item["text"] = plain_gloss(sense["text"], starters, sense.get("pos") == "proper noun")
            if sense.get("note"):
                item["note"] = sense["note"]
            if sense.get("replaced"):
                # the text before a rewrite: search still finds the meaning by it; never shown
                item["replaced"] = plain_gloss(sense["replaced"], starters, sense.get("pos") == "proper noun")
            senses.append(dict(item, id=sense["id"], source="CORE"))
        examples = [
            dict({"chinese": full_width(ex["chinese"])}, **({"pinyin": ex["pinyin"]} if ex.get("pinyin") else {}),
                 english=ex["english"], source="CORE", sense_id=ex["sense"])
            for ex in entry.get("examples", [])
        ]
        core[(simplified, key_of(syllables))] = {
            "syllables": syllables, "traditional": traditional.spellings((simplified, key_of(syllables))),
            "senses": senses, "examples": examples, "classifiers": entry.get("measure_words") or [],
        }
    return core


def read_sources(cedict, wiktionary, english, entries, variants, counts):
    cc, cc_metadata, support_cc = cached("cedict", load_cedict, cedict)
    valid = {letters.lower() for item in cc for letters, _ in item["syllables"]}
    valid |= {"r"}
    print("Reading Wiktionary")
    abbreviations = cached("abbreviations", english_abbreviations, english)
    wikt, written = cached("wiktionary", load_wiktionary, wiktionary, valid, abbreviations)
    words, wikt_new = merge(cc, wikt)
    core, core_use, core_added, core_merged, core_skipped = {}, {}, set(), set(), 0
    if entries:
        print("Reading the curated core")
        attested = {(item["simplified"], tuple(letter.lower() for letter, _ in reading))
                    for item in cc + wikt for reading in item.get("readings") or [item["syllables"]]}
        equivalents = [(item["traditional"], target) for item in cc
                       for target in item.get("glyph_variants", [])
                       if len(item["traditional"]) == len(target) == 1
                       and target in words.get((item["simplified"], key_of(item["syllables"])), {}).get("traditional", [])]
        traditional = Traditional(words, variants, counts, equivalents)
        core = load_core(entries.parent, lowercase_starters(cc + wikt), traditional)
        core_use, core_added, core_merged, core_skipped = add_core(words, core, attested)
    cc_names = {(item["simplified"], key_of(item["syllables"])) for item in cc
                if any(char.isupper() for letters, _ in item["syllables"] for char in letters)}
    attach_written(words, written, cc_names)
    return {"words": words, "wikt_new": wikt_new, "core_entries": len(core), "core_use": core_use,
            "core_added": core_added, "core_merged": core_merged, "core_skipped": core_skipped,
            "cc_metadata": cc_metadata, "support_cc": support_cc}


def merge(cc, wikt):
    words = {}

    def add(item, syllables):
        key = (item["simplified"], key_of(syllables))
        word = words.setdefault(key, {
            "simplified": item["simplified"], "traditional": [], "syllables": syllables,
            "senses": defaultdict(list), "examples": defaultdict(list), "classifiers": [],
            "sources": [], "names": set(), "wiktionary_pages": [],
        })
        if item["source"] not in word["sources"]:
            word["sources"].append(item["source"])
        # a wiktionary entry's traditional spelling is its page title
        if item["source"] == "WIKT" and item["traditional"] not in word["wiktionary_pages"]:
            word["wiktionary_pages"].append(item["traditional"])
        for traditional in [item["traditional"], *item.get("traditional_forms", [])]:
            if traditional not in word["traditional"] and not (
                item["source"] == "WIKT" and traditional == item["simplified"] and word["traditional"]
                and traditional not in item.get("traditional_forms", [])
            ):
                word["traditional"].append(traditional)
        # glosses of a capitalised reading may be names (哀 Ai1 sits with ai1), for search_glosses
        name = item["source"] == "CC" and syllables[0][0][:1].isupper()
        for sense in item["senses"]:
            if name:
                word["names"].add(sense["text"])
            if all(sense["text"] != old["text"] for old in word["senses"][item["source"]]):
                word["senses"][item["source"]].append(sense)
        for example in item["examples"]:
            if all(example["chinese"] != old["chinese"] for old in word["examples"][item["source"]]):
                word["examples"][item["source"]].append(example)
        for classifier in item["classifiers"]:
            if classifier not in word["classifiers"]:
                word["classifiers"].append(classifier)
        if word["syllables"][0][0][:1].isupper() and syllables[0][0][:1].islower():
            word["syllables"] = syllables

    for item in cc:
        add(item, item["syllables"])
    traditional_forms = {item["traditional"] for item in cc} - {item["simplified"] for item in cc}
    traditional_spellings = {item["traditional"] for item in cc if item["traditional"] != item["simplified"]}

    by_headword = defaultdict(list)
    for key in words:
        by_headword[key[0]].append(key)
    added_words = 0
    for item in wikt:
        known = by_headword.get(item["simplified"])
        # only a traditional spelling of a known word (戰 is 战)
        if not known and (item["simplified"] in traditional_forms or item["traditional"] in traditional_spellings):
            continue
        if not known and len(item["simplified"]) == 1 and ord(item["simplified"]) > 0xFFFF and all(
            sense.get("form_of") or EMPTY_MEANING.match(sense["text"]) for sense in item["senses"]
        ):
            continue
        if not known:
            add(item, item["readings"][0])
            by_headword[item["simplified"]].append((item["simplified"], key_of(item["readings"][0])))
            added_words += 1
            continue
        candidates = [r for r in item["readings"] if (item["simplified"], key_of(r)) in words]
        if not candidates:
            continue
        item_words = set().union(*(content_words(s["text"]) for s in item["senses"]))

        def overlap(reading):
            word = words[(item["simplified"], key_of(reading))]
            existing = set()
            for senses in word["senses"].values():
                for sense in senses:
                    existing |= content_words(sense["text"])
            return len(existing & item_words)

        best = max(candidates, key=lambda reading: (overlap(reading), -candidates.index(reading)))
        add(item, best)
    return words, added_words


def add_core(words, core, attested):
    # no exact key: fill a free record that differs only in tone (看见 kan4 jian vs kan4 jian4)
    # a new reading of a known word must be attested, since the core also covers
    # colloquial readings such as 我们 wan3 me
    use, added, merged, skipped = {}, set(), set(), 0
    by_letters = defaultdict(list)

    def letters(key):
        return key[0], tuple(letter for letter, _ in key[1])

    for key in words:
        by_letters[letters(key)].append(key)
    known = {key[0] for key in words}

    use.update({key: curated for key, curated in core.items() if key in words})
    for key, curated in core.items():
        if key in words:
            target = key
        else:
            free = [other for other in by_letters[letters(key)] if other not in use]
            if (not free and by_letters[letters(key)]) or (key[0] in known and letters(key) not in attested):
                skipped += 1
                continue
            if free:
                target = min(free, key=lambda other: sum(a != b for a, b in zip(other[1], key[1])))
                merged.add(target)
            else:
                target = key
                words[key] = {
                    "simplified": key[0], "traditional": [], "syllables": curated["syllables"],
                    "senses": defaultdict(list, CORE=curated["senses"]), "examples": defaultdict(list),
                    "classifiers": [], "sources": ["CORE"], "names": set(),
                }
                by_letters[letters(key)].append(key)
                added.add(key)
            use[target] = curated
        for traditional in curated["traditional"]:
            if traditional not in words[target]["traditional"]:
                words[target]["traditional"].append(traditional)
        if not words[target]["traditional"]:
            raise ValueError(f"No traditional spelling for {target[0]}")
    return use, added, merged, skipped
