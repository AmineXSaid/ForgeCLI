import subprocess

expected = (
    ".PHONY: lint test build\n\nlint:\n\tpython3 -m py_compile app.py\n\n"
    "test:\n\tpython3 -m unittest -v\n\nbuild: test\n\tmkdir -p dist && cp app.py dist/\n"
)
actual = open("Makefile").read()
assert actual == expected, "Makefile differs from the expected result:\n" + repr(actual)
out = subprocess.run(["make", "test"], capture_output=True, text=True)
assert out.returncode == 0, out.stdout + out.stderr
assert "test_greet" in out.stderr, "unittest -v names each test"
print("hidden checks passed")
