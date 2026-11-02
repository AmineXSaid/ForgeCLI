cat > encode.py <<'PY'
"""Encode a file as FZ1 (see decode.py): python3 encode.py IN OUT"""

import sys


def encode(src: bytes) -> bytes:
    out = bytearray(b"FZ1")
    key = 0x5A
    lit = bytearray()

    def flush():
        nonlocal key
        for k in range(0, len(lit), 128):
            run = lit[k : k + 128]
            out.append(len(run) - 1)
            for c in run:
                out.append(c ^ key)
                key = c
        lit.clear()

    i = 0
    while i < len(src):
        best_len, best_d = 0, 0
        for d in range(1, min(255, i) + 1):
            n = 0
            while n < 130 and i + n < len(src) and src[i + n] == src[i + n - d]:
                n += 1
            if n > best_len:
                best_len, best_d = n, d
        if best_len >= 3:
            flush()
            out += bytes([0x80 | (best_len - 3), best_d])
            i += best_len
            key = src[i - 1] ^ best_d
        else:
            lit.append(src[i])
            i += 1
    flush()
    return bytes(out)


if __name__ == "__main__":
    data = open(sys.argv[1], "rb").read()
    open(sys.argv[2], "wb").write(encode(data))
PY
python3 encode.py data.txt data.fz
