---
status: approved
revision: 1
---

# 常驻引导文本加入 `affected`（Q3）

来源：`docs/CLAUDE-USAGE-EVAL-2026-09-28.md` F2 与第 7 节 Q3（本地文档）。2026-09-29 用户批准"按建议执行"。

## goal

"改了 X 该重跑哪些测试"是编程任务里每次改动之后都会问的问题，`code-graph-mcp affected <files>` 正是回答它的命令，但它在常驻引导文本（MCP `instructions`、CLAUDE.md 管理块）里 0 次出现，只在按需读取的详细文档里有。D10A（`80a2aaf`）之后 Python 包内相对导入也能解析，`affected` 的答案在 Python 上从 0 个测试变成了真实的 45 个，值得常驻。

## non-goals

- 不改 `affected` 的行为或输出。
- 不加 `tour`、`--budget`（F2 的另一半，评测没有显示需求）。
- 不改 code-explorer agent（它做探索，不选测试）。

## 设计

- MCP `instructions`（noisy）：在 impact 之后加 "which tests load a changed file → `code-graph-mcp affected <file>`"。
- MCP `instructions`（quiet）：命令列表加 `affected <file>`。
- CLAUDE.md 管理块（`adopt.js` `buildTriggerRows`，只在显式 `adopt` 时写入）：impact 行之后加一行；`tests/routing_bench.rs` 的镜像同步。

## constraints

- noisy ≤ 1500 字节、quiet ≤ 400 字节（编译期断言）。
- 文本里每个 `code-graph-mcp <cmd>` 都要过 `tests/doc_cli_alignment.rs` 的 CLI 对齐检查。

## success-criteria

- 三处文本都含 `affected`，并有测试钉住；对齐检查与镜像漂移检查通过。
- 字节数：noisy 975 → 修改后数值记录在提交信息里。

## open-questions

- 效果（模型是否因此调用 `affected`）需要付费评测才能测，本次不跑；留给下一轮编程评测。

# Change log

- r1（2026-09-29）：初稿。
