import inspect
import sys
sys.path.insert(0, ".")
import text
from text import slugify
assert slugify("  --Leading and trailing--  ") == "leading-and-trailing", slugify("  --Leading and trailing--  ")
assert slugify("Ünïcödé Ünïcödé") == "unicode-unicode"
assert slugify("tabs\tand\nnewlines") == "tabs-and-newlines"
src = inspect.getsource(text.slugify)
assert "re.compile" not in src, "the patterns should be compiled once, outside slugify()"
print("hidden checks passed")
