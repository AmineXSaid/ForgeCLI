python3 - <<'PY'
s = open("pack.py").read()
start = s.index("def pack(weights):")
end = s.index('if __name__ == "__main__":')
first_fit = '''def pack(weights):
    """First fit: put each item in the first open bin with room for it."""
    bins = []
    for w in weights:
        for b in bins:
            if sum(b) + w <= CAPACITY:
                b.append(w)
                break
        else:
            bins.append([w])
    return bins


'''
open("pack.py", "w").write(s[:start] + first_fit + s[end:])
PY
python3 check.py
python3 pack.py items.txt | wc -l | tr -d ' ' > bins.txt
