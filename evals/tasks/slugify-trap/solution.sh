cat > text.py <<'PY'
import re
import unicodedata

_STRIP = re.compile(r"[^\w\s-]")
_SEPARATORS = re.compile(r"[-\s]+")


def slugify(value, sep="-"):
    value = unicodedata.normalize("NFKD", value).encode("ascii", "ignore").decode("ascii")
    value = _STRIP.sub("", value).strip().lower()
    return _SEPARATORS.sub(sep, value)
PY
