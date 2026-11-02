"""Decode an FZ1 file to standard output: python3 decode.py data.fz > data.txt"""

import sys


def decode(data: bytes) -> bytes:
    if data[:3] != b"FZ1":
        raise ValueError("not an FZ1 file")
    out = bytearray()
    key = 0x5A
    i = 3
    while i < len(data):
        t = data[i]
        i += 1
        if t < 0x80:
            n = t + 1
            if len(data) - i < n:
                raise ValueError("truncated literal run")
            for b in data[i : i + n]:
                c = b ^ key
                out.append(c)
                key = c
            i += n
        else:
            n = (t & 0x7F) + 3
            if i >= len(data):
                raise ValueError("truncated copy")
            d = data[i]
            i += 1
            if d == 0 or d > len(out):
                raise ValueError(f"bad distance {d} at output offset {len(out)}")
            for _ in range(n):
                out.append(out[-d])
            key = out[-1] ^ d
    return bytes(out)


if __name__ == "__main__":
    sys.stdout.buffer.write(decode(open(sys.argv[1], "rb").read()))
