python3 - <<'PY'
s = open("cli.py").read()
s = s.replace("import argparse\n", "import argparse\nimport os\n\nHERE = os.path.dirname(os.path.abspath(__file__))\n")
s = s.replace(
    '    parser.add_argument("name", nargs="?", default="world")\n',
    '    parser.add_argument("name", nargs="?", default="world")\n'
    '    with open(os.path.join(HERE, "VERSION")) as f:\n'
    '        version = f.read().strip()\n'
    '    parser.add_argument("--version", action="version", version=f"%(prog)s {version}")\n',
)
open("cli.py", "w").write(s)
PY
cat > tests/test_version.py <<'PY'
import contextlib
import io
import unittest

from cli import main


class VersionTest(unittest.TestCase):
    def test_version(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), self.assertRaises(SystemExit):
            main(["--version"])
        self.assertIn("1.4.2", out.getvalue())
PY
