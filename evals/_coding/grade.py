"""Grade the coding cases on the workspaces `claude plugin eval --keep-temp` left.

    python3 evals/_coding/grade.py [--suite] [--out FILE] RESULT.json

RESULT.json is the file `evals/run.sh ... --keep-temp --json RESULT.json`
wrote. Each run's kept directory is the parent of its tracePath; the eval
seals home/ and tmp/ into <dir>/sealed/ (mode 000), which this script opens.
The workspace is copied out before any test runs, so nothing executes inside
the kept directory. For each run of a code-* case it then:

- runs the case's hidden tests against the workspace (hidden/<case>/),
  or, for code-dead-helpers, compares the deleted functions with the gold set;
- with --suite, also runs the whole networkx test suite in the workspace, so a
  fix that breaks something else is counted;
- reads the transcript for what the agent did: turns, tool calls, code-graph
  CLI and MCP calls, and what the plugin's hooks put into its context.

Prints one JSON object per run (and writes them all to --out).
"""

import argparse
import json
import re
import subprocess
import sys
import tempfile
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
EVALS = HERE.parent
CACHE = Path("/var/tmp/code-graph-eval/coding")

CG_CLI = re.compile(
    r"code-graph-mcp\s+(callgraph|impact|refs|show|search|grep|overview|map|tour|deps|"
    r"dead-code|affected|ast-search|health-check|explain|trace|similar)\b"
)
CG_MARK = re.compile(r"code-graph|\[cg[:\]]|code_graph")


def case_prompts():
    out = {}
    for prompt in EVALS.glob("code-*/prompt.md"):
        body = prompt.read_text().split("---", 2)[2].strip()
        out[prompt.parent.name] = " ".join(body.split())[:80]
    return out


def main_transcript(run_dir):
    files = [
        p
        for p in (run_dir / "config" / "projects").glob("*/*.jsonl")
        if "subagents" not in p.parts
    ]
    return max(files, key=lambda p: p.stat().st_size) if files else None


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            c.get("text", "") if isinstance(c, dict) else str(c) for c in content
        )
    return ""


def read_transcript(path, prompts):
    stats = Counter()
    tools = Counter()
    cg_cli = Counter()
    cg_mcp = Counter()
    hook_chars = Counter()
    hook_count = Counter()
    case = None
    first_ts = last_ts = None
    usage = Counter()
    seen_req = set()
    for line in path.read_text().splitlines():
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        ts = ev.get("timestamp")
        if ts:
            first_ts = first_ts or ts
            last_ts = ts
        msg = ev.get("message") or {}
        if ev.get("type") == "user" and case is None:
            t = " ".join(text_of(msg.get("content")).split())
            for name, head in prompts.items():
                if head[:60] and head[:60] in t:
                    case = name
        if ev.get("type") == "assistant":
            rid = ev.get("requestId") or msg.get("id")
            if rid and rid not in seen_req:
                seen_req.add(rid)
                stats["assistant_requests"] += 1
                for k, v in (msg.get("usage") or {}).items():
                    if isinstance(v, int):
                        usage[k] += v
            for c in msg.get("content") or []:
                if not isinstance(c, dict) or c.get("type") != "tool_use":
                    continue
                name = c.get("name", "")
                tools[name] += 1
                if name.startswith("mcp__") and "code-graph" in name:
                    cg_mcp[name.rsplit("__", 1)[-1]] += 1
                if name == "Bash":
                    cmd = (c.get("input") or {}).get("command", "")
                    for m in CG_CLI.finditer(cmd):
                        cg_cli[m.group(1)] += 1
        if ev.get("type") == "attachment":
            att = ev.get("attachment") or {}
            blob = json.dumps(att)
            if CG_MARK.search(blob):
                ev_name = att.get("hookEvent") or att.get("type") or "?"
                hook_count[ev_name] += 1
                hook_chars[ev_name] += len(blob)
    return {
        "case": case,
        "assistant_requests": stats["assistant_requests"],
        "tools": dict(tools),
        "tool_calls": sum(tools.values()),
        "cg_cli": dict(cg_cli),
        "cg_mcp": dict(cg_mcp),
        "cg_calls": sum(cg_cli.values()) + sum(cg_mcp.values()),
        "cg_hook_attachments": dict(hook_count),
        "cg_hook_chars": dict(hook_chars),
        "usage": dict(usage),
        "first_ts": first_ts,
        "last_ts": last_ts,
    }


def run_pytest(workspace, target, timeout):
    proc = subprocess.run(
        [sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", str(target)],
        cwd=workspace,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    tail = proc.stdout.strip().splitlines()[-1:] or [""]
    counts = {
        k: int(n) for n, k in re.findall(r"(\d+) (passed|failed|error|errors)", tail[0])
    }
    return {"rc": proc.returncode, "summary": tail[0], **counts}


def grade_run(run_dir, case, arm, meta, prompts, suite, scratch):
    run_dir = Path(run_dir)
    for d in (run_dir, run_dir / "sealed"):
        if d.exists():
            d.chmod(0o700)
    home = run_dir / "sealed" / "home"
    manifest = home / ".cache" / "code-graph" / "install-manifest.json"
    tr = main_transcript(run_dir)
    info = read_transcript(tr, prompts) if tr else {}
    out = {
        "run_dir": str(run_dir),
        "case": case,
        "arm": arm,
        **meta,
        **info,
        "manifest": manifest.exists(),
    }
    out["case"] = case
    src = home / "cwd"
    if not src.is_dir():
        out["error"] = "workspace not found"
        return out
    workspace = Path(scratch) / run_dir.name
    subprocess.run(["rm", "-rf", str(workspace)], check=True)
    subprocess.run(["cp", "-a", str(src), str(workspace)], check=True)
    if case == "code-dead-helpers":
        with tempfile.TemporaryDirectory() as fx:
            subprocess.run(
                ["tar", "-xf", str(CACHE / "fixtures" / f"{case}.tar"), "-C", fx],
                check=True,
            )
            proc = subprocess.run(
                [
                    sys.executable,
                    str(HERE / "hidden" / case / "dead_helpers.py"),
                    "grade",
                    fx,
                    str(workspace),
                ],
                capture_output=True,
                text=True,
                check=True,
            )
        g = json.loads(proc.stdout)
        out["hidden"] = g
        out["score"] = g["recall"] if not g["false_positives"] else 0.0
    else:
        h = run_pytest(workspace, HERE / "hidden" / case, 600)
        out["hidden"] = h
        total = (
            h.get("passed", 0)
            + h.get("failed", 0)
            + h.get("error", 0)
            + h.get("errors", 0)
        )
        out["score"] = round(h.get("passed", 0) / total, 3) if total else 0.0
    if suite:
        out["suite"] = run_pytest(workspace, "networkx", 1800)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("result_json")
    ap.add_argument("--suite", action="store_true")
    ap.add_argument("--out")
    ap.add_argument("--scratch", default="/var/tmp/code-graph-eval/graded")
    ap.add_argument("-j", "--jobs", type=int, default=4)
    args = ap.parse_args()
    prompts = case_prompts()
    Path(args.scratch).mkdir(parents=True, exist_ok=True)
    results = []
    jobs = []
    data = json.loads(Path(args.result_json).read_text())
    for case in data["cases"]:
        if not case["name"].startswith("code-"):
            continue
        for arm, runs in case["arms"].items():
            for i, run in enumerate(runs):
                meta = {
                    k: run.get(k)
                    for k in ("turns", "costUsd", "durationSeconds", "error")
                }
                meta["run"] = i
                trace = run.get("tracePath")
                if not trace:
                    results.append(
                        {"case": case["name"], "arm": arm, **meta, "error": "no trace"}
                    )
                    continue
                jobs.append((Path(trace).parent.parent, case["name"], arm, meta))
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = [
            pool.submit(grade_run, d, c, a, m, prompts, args.suite, args.scratch)
            for d, c, a, m in jobs
        ]
        for fut in futures:
            res = fut.result()
            results.append(res)
            print(json.dumps(res), flush=True)
    if args.out:
        Path(args.out).write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()
