---
status: approved
revision: 1
---

# `impact` / `callgraph` 加 `--node-id`（Q4）

来源：`docs/CLAUDE-USAGE-EVAL-2026-09-28.md` 第 7 节 Q4（本地文档）。2026-09-29 用户批准"按建议执行"。

## goal

`174f9b6` 之后，一个文件里有多个同名定义（Python 多个类的 `__init__`、`get`，Rust 多个 `impl` 的 `new`）时，`impact X --file F` 与 `callgraph X --file F` 拒答并列出各定义的 `node_id`，但这两个命令没有办法按 `node_id` 回答；编辑前 hook 因此对这类编辑静默。

给两个命令加 `--node-id N`（与 `show`、`refs` 同义：`node_id` 说了算，`--file` 被忽略并提示），编辑前 hook 在拒答时按被编辑定义所在行选出节点再问一次。

## non-goals

- MCP `get_call_graph` 不加 `node_id`（本次只改 CLI；MCP 侧已有 `get_ast_node`/`find_references` 走 node_id）。
- 不改名字查询的任何行为。

## 设计

- `graph::query`：遍历的种子抽成 `CallGraphSeed`（名字 + 文件，或节点 id）；按名字的函数变成它的包装，MCP、`trace` 不变。
- `impact` / `callgraph`：`symbol` 在给了 `--node-id` 时可省略；找不到该 id 时退出 1 并给出带 `node_id` 的 JSON 错误；查询时刷新（`refresh_files_if_stale`）若重建了该文件，按 (文件, 名字, 限定名, 类型) 重新找回节点（与 `refs --node-id` 同一规则，SURF-16），找不到则退出 1。
- 编辑前 hook：`impact --file` 的同文件拒答带回候选的 `node_id` 与 `start_line`；取起始行不晚于被编辑定义头所在行的最后一个候选，再以 `--node-id` 查询。

## success-criteria

- 两个同名定义各有不同调用者时，`--node-id` 只报该定义的调用者（`impact` 与 `callgraph`，文本与 JSON）。
- 刷新导致 id 被复用时，仍回答原来那个定义（含防空过检查：旧 id 确实被别的符号占用）。
- 编辑前 hook 对同文件同名方法的签名编辑会注入所编辑那个方法的影响。
- 每条新分支都有变异被抓。

## open-questions

- 无。

# Change log

- r1（2026-09-29）：初稿。
