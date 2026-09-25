# scripts/embedding_benchmark/leakage.py
"""Query-to-document leakage for the bootstrap (doc -> code) query set. Stdlib only.

A bootstrap query IS a symbol's doc comment, and the binary writes that same doc
into the symbol's embedded text (`src/embedding/context.rs`: `doc: {doc}`). Scored
on `context_string`, such a query finds its gold by near-verbatim overlap, which
measures string matching, not doc -> code retrieval: the 2026-06-21 NDCG@10 of
0.8655 was taken that way. These helpers give the evaluator a field without the
doc and a check that says how much of a run is leaked, so the next reading of a
number carries the fact instead of relying on someone to remember it.
"""
import re

# `build_context_string` joins its parts with "\n"; the doc part is `doc: …` and
# may itself span lines, so it runs up to the next part (`code: ` is the only one
# that can follow it) or the end of the string.
_DOC_PART = re.compile(r"(?:^|\n)doc: .*?(?=\ncode: |\Z)", re.S)

# How many leading query words must appear contiguously in the document for the
# query to count as leaked. Twelve words of a doc comment reproduced verbatim is
# not a coincidence; shorter docs are compared whole.
LEAK_WINDOW = 12


def strip_doc(context_string: str) -> str:
    """`context_string` without its `doc:` part."""
    return _DOC_PART.sub("", context_string or "")


def words(text: str) -> list[str]:
    return re.findall(r"[a-z0-9]+", (text or "").lower())


def _contains_run(hay: list[str], needle: list[str]) -> bool:
    if not needle or len(needle) > len(hay):
        return False
    n = len(needle)
    first = needle[0]
    return any(hay[i] == first and hay[i:i + n] == needle for i in range(len(hay) - n + 1))


def is_leaked(query: str, document: str) -> bool:
    """True when the query's first LEAK_WINDOW words appear, in order and
    contiguously, in the document (after lowercasing and dropping punctuation)."""
    return _contains_run(words(document), words(query)[:LEAK_WINDOW])
