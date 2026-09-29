---
status: proposed
revision: 1
---

# JS/TS：赋值式函数与类箭头字段建节点，CJS 目录引用可解析

来源：`docs/CLAUDE-USAGE-EVAL-2026-09-28.md` C1–C3（本地文档，docs/ 不入库；要点抄录如下）。

## goal

JS/TS 项目里最常见的几种"定义函数"的写法现在不产生节点，于是 callgraph/impact/refs/affected 对这些项目给出错误或空的答案，hook 还会把"Risk: LOW, 0 callers"注入给 Claude。让这些写法和 `function f() {}` 一样成为可查询的节点，调用能解析到它们。

2026-09-28 实测（v0.163.0，R）：

- express 4.21.2 `lib/`：`x.y = function`、`X.prototype.m = function` 形式 70 个，已索引的函数 37 个，缺失 70/107 = 65%。`show send` → "Symbol not found … Did you mean: sendfile"（另一个函数）。callgraph 调用者精确率 1/4、召回 1/8。
- hono：`Context` 的 15 个箭头字段（`json/text/html/header/status/redirect/body/render/set/get/notFound/newResponse/setLayout/getLayout/setRenderer`）不在库里；`callgraph json` 回答的是 `HonoRequest.json`，4 个生产调用者只有 1 个对。
- express `affected lib/view.js` 报 1 个测试文件，实际 74/93 个测试文件经 `require('..')` 加载了该包；`deps index.js` 把 `module.exports = require('./lib/express')` 说成 "<external>"。

代码位置（C）：`src/parser/treesitter.rs` 的 `"arrow_function" | "function_expression"` 分支只在 `route_handler_name` 命中时建节点；解析器里没有 `public_field_definition` 的处理。

## non-goals

- 动态属性名（`obj[name] = function`）、`Object.assign(proto, {...})`、`defineProperty` 的 getter：本轮不建节点，记为已知盲区。
- 从外部包 `require('express')` 解析到 node_modules：不做。
- 改 `callgraph`/`impact` 的输出格式或默认置信度下限。

## 接受形态表（先写成语料测试，确认失败，再实现）

| 形态 | 节点名 / qualified_name | 类型 |
|---|---|---|
| `res.send = function send(body) {}` | `send` / `res.send` | function |
| `res.json = function (obj) {}`（匿名） | `json` / `res.json` | function |
| `View.prototype.lookup = function lookup(name) {}` | `lookup` / `View.lookup` | method |
| `exports.normalizeType = function (type) {}` | `normalizeType` / `exports.normalizeType` | function |
| `module.exports.f = () => {}` | `f` / `exports.f` | function |
| `app.handle = function handle(req, res) {}`（`app = exports = module.exports = {}`） | `handle` / `app.handle` | function |
| TS `class Context { json: JSONRespond = (obj) => {…} }` | `json` / `Context.json` | method |
| TS `class C { private readonly f = function () {} }` | `f` / `C.f` | method |
| `x.y.z = function () {}`（多级） | `z` / `x.y.z` | function |
| 不接受：`obj[k] = function`、`a.b = someVar`（右侧不是函数字面量）、`a.b = require('x')` | — | — |

CJS 解析（C3）：

| 形态 | 解析到 |
|---|---|
| `require('..')` / `require('../')` | 父目录的 `index.js`，或 `package.json#main` |
| `require('./lib/express')`（无扩展名，目录或文件） | `lib/express.js` 或 `lib/express/index.js` |
| `module.exports = require('./lib/express')` | 把该文件的默认导出视为转出 `lib/express.js` 的导出（再导出） |

## constraints

- 开放输入域：先写接受形态表对应的语料测试（上表每行一个用例）并确认失败，再改实现（记忆 feedback_corpus_first_and_a_review_stop_line）。评审第 2 轮仍在上一轮修复里出 HIGH 就停下重新设计。
- 新节点会进入同名解析：`send`、`json`、`handle`、`get`、`set` 是高频名字，会增加 ambiguous 扇出。**先缩小结果、不缩小候选池**（记忆 feedback_narrow_the_result_not_the_pool）；用 SCIP 预言机量前后：`scripts/scip_oracle/run.sh --language javascript` 与 `corpora.sh`（hono、express），要求 0 个新增错误边、0 个丢失的正确边，报告 extracted/inferred/ambiguous 各层精确率与召回的前后数字。
- 增量与全量一致：改一个文件、删一个文件、改名，两条路径导出的节点与边逐行相同（记忆 project_qa_sweep_2026_09_14 的 4 路编辑配方）。
- `INDEX_VERSION` 升级（全体用户首次使用时自动重建一次），CHANGELOG 写明升级影响与回退方式，沿用 0.163.0 的写法。
- 属于索引内容变化、修复文档已承诺的 JS/TS 支持：按 fix 处理，L2。

## 验收

- 语料测试全绿；express `show send` 命中 `res.send`；hono `callgraph json --file src/context.ts` 返回 `c.json(...)` 的调用者。
- express `callgraph` 调用者精确率/召回从 1/4、1/8 提升，hono 从 4/7、4/6 提升（同一查询集，见本地 `/var/tmp/cg-usage-eval/work/query/out/`，或重建）。
- express `affected lib/view.js` 列出 ≥70 个测试文件，或在有未解析导入时明确说"至少 N 个，M 个导入未解析"。
- SCIP 预言机 JS 语料：不新增错误边、不丢正确边。
