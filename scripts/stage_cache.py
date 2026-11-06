"""Cache slow build stages as pickles.

A stage's key hashes every script in scripts/ except build_dictionary.py, the data files the
helpers read (stop words, kept traditional spellings, English usage), the Python version, the function and
its arguments (files by content). So editing a helper or a data input reruns the stages and editing
build_dictionary.py never does. Results must depend only on the arguments and these files.
"""

import gc
import hashlib
import inspect
import os
import pickle
import sys
from pathlib import Path

from stopwords import PATH as WORD_LIST
from traditional import KEPT_FILE
from usual_words import USAGE as USAGE_LIST

KEPT = 3  # results per stage, newest use first, so checkouts sharing a cache don't wipe each other's
FORMAT = 1  # bump when the stored layout changes
PROTOCOL = pickle.HIGHEST_PROTOCOL
SCRIPTS = Path(__file__).resolve().parent
folder = None
_files = {}


def file_sha256(path):
    # rehashed only when the file's metadata changes
    stat = path.stat()
    known = (path.resolve(), stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns)
    if known not in _files:
        with path.open("rb") as stream:
            _files[known] = hashlib.file_digest(stream, "sha256").hexdigest()
    return _files[known]


def stable(value):
    # same text every run; set order changes between runs
    if isinstance(value, Path):
        if value.is_dir():
            files = [p for p in sorted(value.rglob("*")) if p.is_file()]
            return f"dir({[(p.relative_to(value).as_posix(), file_sha256(p)) for p in files]})"
        return f"file({file_sha256(value) if value.exists() else None})"
    if isinstance(value, (set, frozenset)):
        return "{" + ",".join(sorted(map(stable, value))) + "}"
    if isinstance(value, dict):
        return "{" + ",".join(f"{stable(k)}:{stable(v)}" for k, v in value.items()) + "}"
    if isinstance(value, (list, tuple)):
        return type(value).__name__ + "(" + ",".join(map(stable, value)) + ")"
    return repr(value)


LATE = "build_dictionary.py"  # never part of a key, so it can hold no stages


def code_files():
    scripts = [path for path in sorted(SCRIPTS.glob("*.py")) if path.name != LATE]
    return scripts + [WORD_LIST, KEPT_FILE, USAGE_LIST]


def cached(name, function, *args):
    if folder is None:
        return function(*args)
    if Path(inspect.getsourcefile(function)).name == LATE:
        raise ValueError(f"stage {name}: move {function.__name__} out of {LATE}, which the key leaves out")
    code = [(path.name, file_sha256(path)) for path in code_files()]
    inputs = stable(args)
    key = hashlib.sha256(repr((FORMAT, PROTOCOL, sys.version, code, function.__qualname__, inputs)).encode())
    path = folder / f"{name}-{key.hexdigest()[:32]}.pickle"
    try:
        # results live until the build ends; stop the gc rescanning them
        gc.disable()
        with path.open("rb") as stream:
            result = pickle.load(stream)
        os.utime(path)
        gc.freeze()
        return result
    except FileNotFoundError:  # not cached yet, or just pruned by a parallel build
        pass
    finally:
        gc.enable()
    result = function(*args)
    if stable(args) != inputs:
        print(f"Warning: inputs of stage {name} changed while it ran; not caching it", file=sys.stderr)
        return result
    folder.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(f".{os.getpid()}.part")
    with partial.open("wb") as stream:
        pickle.dump(result, stream, protocol=PROTOCOL)
    partial.replace(path)
    os.utime(path)
    used = []
    for old in folder.glob(f"{name}-{'?' * 32}.pickle"):
        try:
            used.append((old.stat().st_mtime_ns, old == path, old))  # the new result wins a tie
        except FileNotFoundError:  # pruned by a parallel build
            pass
    for _, _, old in sorted(used, reverse=True)[KEPT:]:
        old.unlink(missing_ok=True)
    return result
