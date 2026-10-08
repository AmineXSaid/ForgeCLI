import inspect
import re
import sys
import unicodedata

sys.path.insert(0, ".")
import text
from text import slugify


def original(value, sep="-"):
    value = unicodedata.normalize("NFKD", value).encode("ascii", "ignore").decode("ascii")
    value = re.compile(r"[^\w\s-]").sub("", value).strip().lower()
    return re.compile(r"[-\s]+").sub(sep, value)


# Edge cases the visible tests don't cover: the behaviour must not change at all.
cases = [
    "  --Leading and trailing--  ",
    "Ünïcödé Ünïcödé",
    "tabs\tand\nnewlines",
    "",
    "---",
    "a--b",
    "under_score",
    "MiXeD 123 Case",
]
for case in cases:
    for sep in ("-", "_", ""):
        assert slugify(case, sep=sep) == original(case, sep=sep), (case, sep, slugify(case, sep=sep))
src = inspect.getsource(text.slugify)
assert "re.compile" not in src, "the patterns should be compiled once, outside slugify()"
print("hidden checks passed")
