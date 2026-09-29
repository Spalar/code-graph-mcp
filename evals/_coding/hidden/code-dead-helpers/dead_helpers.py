"""Gold set and grader for code-dead-helpers.

The task: delete module-level private functions under networkx/algorithms/
and networkx/generators/ (test files excluded) that nothing references.

gold(root)   -> the functions whose name occurs nowhere in the repo's text
               files except on their own `def` line. A name defined more than
               once in scope is left out of both gold and false positives:
               the text count cannot tell the twins apart.
grade(fixture, workspace) -> which gold functions are gone (recall), and
               which referenced functions were deleted (false positives).

    python3 dead_helpers.py gold <root>
    python3 dead_helpers.py grade <fixture_root> <workspace_root>
"""

import ast
import json
import re
import sys
from collections import Counter
from pathlib import Path

SCOPES = ("networkx/algorithms", "networkx/generators")
TEXT_SUFFIXES = {".py", ".rst", ".md", ".txt", ".toml", ".cfg", ".ipynb", ".ini"}


def _in_scope(rel):
    return rel.startswith(SCOPES) and "/tests/" not in rel and rel.endswith(".py")


def private_defs(root):
    """{(relpath, name)} for module-level `def _name` in scope."""
    out = set()
    for path in sorted(Path(root).rglob("*.py")):
        rel = path.relative_to(root).as_posix()
        if not _in_scope(rel):
            continue
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"))
        except (SyntaxError, UnicodeDecodeError):
            continue
        for node in tree.body:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if node.name.startswith("_") and not node.name.startswith("__"):
                    out.add((rel, node.name))
    return out


def _corpus(root):
    parts = []
    for path in Path(root).rglob("*"):
        if ".git" in path.parts or not path.is_file():
            continue
        if path.suffix not in TEXT_SUFFIXES:
            continue
        try:
            parts.append(path.read_text(encoding="utf-8"))
        except (UnicodeDecodeError, OSError):
            continue
    return "\n".join(parts)


def gold(root):
    defs = private_defs(root)
    counts = Counter(name for _, name in defs)
    text = _corpus(root)
    dead, twins = set(), set()
    for rel, name in defs:
        if counts[name] > 1:
            twins.add((rel, name))
            continue
        hits = len(re.findall(rf"\b{re.escape(name)}\b", text))
        if hits <= 1:  # only its own def line
            dead.add((rel, name))
    return dead, twins


def grade(fixture, workspace):
    dead, twins = gold(fixture)
    before = private_defs(fixture)
    after = private_defs(workspace)
    removed = before - after
    hit = removed & dead
    false_pos = removed - dead - twins
    return {
        "gold": sorted(f"{r}:{n}" for r, n in dead),
        "removed_gold": sorted(f"{r}:{n}" for r, n in hit),
        "recall": round(len(hit) / len(dead), 3) if dead else None,
        "false_positives": sorted(f"{r}:{n}" for r, n in false_pos),
        "twins_removed": sorted(f"{r}:{n}" for r, n in removed & twins),
    }


if __name__ == "__main__":
    if sys.argv[1] == "gold":
        dead, twins = gold(sys.argv[2])
        print(
            json.dumps({"dead": sorted(map(list, dead)), "twins": len(twins)}, indent=1)
        )
    else:
        print(json.dumps(grade(sys.argv[2], sys.argv[3]), indent=1))
