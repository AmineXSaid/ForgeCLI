import pathlib

text = pathlib.Path("release.txt").read_text().strip()
assert text == "BETA-7731", f"release.txt holds {text!r}, not the beta channel's code"
assert pathlib.Path("config/channels.ini").read_text().count("code =") == 3, "channels.ini unchanged"
print("hidden checks passed")
