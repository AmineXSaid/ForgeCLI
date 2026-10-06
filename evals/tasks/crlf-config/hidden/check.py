data = open("config.ini", "rb").read()
expected = b"[network]\r\nhost = example.org\r\ntimeout = 45\r\n\r\n[cache]\r\ntimeout = 30\r\nsize = 512\r\n"
assert data == expected, f"config.ini differs from the expected bytes:\n{data!r}"
print("hidden checks passed")
