import re
import unicodedata


def slugify(value, sep="-"):
    value = unicodedata.normalize("NFKD", value).encode("ascii", "ignore").decode("ascii")
    value = re.compile(r"[^\w\s-]").sub("", value).strip().lower()
    return re.compile(r"[-\s]+").sub(sep, value)
