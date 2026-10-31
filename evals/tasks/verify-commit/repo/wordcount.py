"""Word and line counts for plain text."""


def count_words(text):
    """The number of words in `text`, separated by any whitespace."""
    return len([w for w in text.split(" ") if w != ""])


def count_lines(text):
    """The number of lines in `text`; a final line without a newline counts."""
    if not text:
        return 0
    return text.count("\n") + (0 if text.endswith("\n") else 1)
