"""Synthetic transcripts for subagent_share.py: python3 -m unittest scripts/test_subagent_share.py"""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import subagent_share as ss  # noqa: E402


def tool(name, **inp):
    return {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": name, "input": inp}]}}


class ClassifyTest(unittest.TestCase):
    def test_bash(self):
        self.assertEqual(ss.classify_bash("code-graph-mcp callgraph foo"), "cg")
        self.assertEqual(ss.classify_bash("cd x && ~/.cache/code-graph/bin/code-graph-mcp show f"), "cg")
        self.assertEqual(ss.classify_bash("grep -rn foo src/"), "search")
        self.assertEqual(ss.classify_bash("LC_ALL=C rg foo | head -5"), "search")
        self.assertEqual(ss.classify_bash("cargo test && echo ok"), None)
        self.assertEqual(ss.classify_bash("git log -- code-graph-mcp.md"), None, "a file name is not the command")

    def test_tools(self):
        self.assertEqual(ss.classify_tool("Grep", {}), "search")
        self.assertEqual(ss.classify_tool("mcp__plugin_code-graph-mcp_code-graph__get_call_graph", {}), "cg")
        self.assertEqual(ss.classify_tool("Edit", {}), None)


class CollectTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.projects = os.path.join(self.tmp.name, "projects")
        self.repo = os.path.join(self.tmp.name, "repo")
        os.makedirs(self.repo)

    def tearDown(self):
        self.tmp.cleanup()

    def agent(self, name, atype, records, cwd=None, ts="2026-09-28T17:00:00Z", hook=None):
        d = os.path.join(self.projects, "-repo", "sess", "subagents")
        os.makedirs(d, exist_ok=True)
        head = {"type": "user", "cwd": cwd or self.repo, "timestamp": ts, "message": {"content": "go"}}
        lines = [head]
        if hook is not None:
            lines.append({"attachment": {"hookEvent": "SubagentStart", "stdout": hook}})
        with open(os.path.join(d, f"agent-{name}.jsonl"), "w") as f:
            f.write("\n".join(json.dumps(r) for r in lines + records) + "\n")
        with open(os.path.join(d, f"agent-{name}.meta.json"), "w") as f:
            json.dump({"agentType": atype}, f)

    def test_groups_by_type_and_delivery(self):
        facts = '{"hookSpecificOutput":{"additionalContext":"This repository has a code-graph AST index of 9 files."}}'
        self.agent("a", "Explore", [tool("Bash", command="code-graph-mcp callgraph f"), tool("Grep")], hook=facts)
        self.agent("b", "Explore", [tool("Grep"), tool("Read"), tool("Edit")])
        self.agent("c", "Explore", [tool("Grep")], hook="")  # the hook ran and said nothing
        self.agent("d", "Explore", [tool("Grep")], cwd=os.path.join(self.tmp.name, "elsewhere"))
        self.agent("e", "Plan", [tool("Glob")], ts="2026-09-27T00:00:00Z")
        g = ss.collect(self.repo, self.projects)
        self.assertEqual(dict(g[("Explore", True)]), {"subagents": 1, "used_cg": 1, "cg": 1, "search": 1})
        self.assertEqual(dict(g[("Explore", False)]), {"subagents": 2, "used_cg": 0, "cg": 0, "search": 3})
        self.assertEqual(g[("Plan", False)]["subagents"], 1)
        self.assertNotIn(("Plan", False), ss.collect(self.repo, self.projects, since="2026-09-28"))


if __name__ == "__main__":
    unittest.main()
