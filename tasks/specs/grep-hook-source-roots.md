---
status: proposed
revision: 1
---

# grep hook：源码目录从索引推导，不再只认写死的目录名

来源：`docs/CLAUDE-USAGE-EVAL-2026-09-28.md` F1（本地文档，要点抄录如下）。

## goal

grep 就地回答是唯一被真实会话证明有效的 hook 机制：被回答后同一搜索 3 步内重复执行 1.2%（4/346），未干预的 grep 为 9.3%（1926/20641）。但它只对路径以写死目录名开头的命令生效（`pre-grep-guide.js` 的 `SRC_PREFIXES`：`src|tests|lib|…|web`）。Python 最常见的布局"目录名 = 包名"（networkx、django、requests、flask 以外的多数项目）完全不在其中。

2026-09-28 编程评测（R）：networkx 上 15 次有插件运行，模型的搜索形态是 `grep -rn "def __new__" networkx/classes/`、`cd networkx/algorithms/flow; grep -n capacity *.py`，grep hook 的记录为 0 条。hook 审计（R）：django 上每一条 grep 都不被接管，`django/` 不在前缀表里。

让"索引里确实有源码文件的顶层目录"与前缀表同等对待。

## non-goals

- 放宽改写语法（`rewritePlan` 的窄语法与"改写必须等价"的约束不变，记忆 feedback_a_rewrite_that_reports_success_must_be_equivalent）。
- `.` 与不写路径的 grep：维持原样（理由见 grep-hook-bare-src-dir.md）。
- `cd 子目录; grep … *.py` 这类以 `cd` 开头的复合命令：维持原样（grep 不是第一段）。

## 设计

- 索引写入时（全量与增量）在 `.code-graph/` 下写一份顶层源码目录清单（例如 `source-roots.json`：含已索引源码文件的顶层目录名，排除测试专用目录之外不做判断），由 Rust 索引器生成，hook 只读。
- `pre-grep-guide.js`：`SRC_PATH` / `SRC_PATH_TOKEN` / `SRC_BARE_TOKEN` 由 `SRC_PREFIXES ∪ source-roots` 构造；清单缺失或损坏时退回现有前缀表（行为与今天相同）。
- 目录名需做正则转义；清单条目数设上限（例如 64），超过则只用前缀表。

## 接受形态表（先写成语料测试）

| 形态（networkx 布局） | 现在 | 之后 |
|---|---|---|
| `grep -rn "X_y" networkx/` | 不接管 | 接管（与 `src/` 同规则） |
| `grep -rn "X_y" networkx/algorithms/flow/` | 不接管 | 接管 |
| `rg "X_y" networkx` | 不接管 | 接管（裸目录规则，见 D#73） |
| `grep -rn "X_y" doc/`（索引里没有源码文件） | 不接管 | 不接管 |
| 清单缺失 | — | 与今天完全相同 |
| 目录名含正则元字符（`c++/`） | — | 转义后匹配，不报错 |

## constraints

- 先写语料测试并确认失败（记忆 feedback_corpus_first_and_a_review_stop_line）。
- 改写输出比原 grep 大 1.5–2.2 倍（hook 审计 P2-1），扩大覆盖前先评估是否对命中数很多的改写降级为只注入：Bash 30K 字符上限下可见命中会从约 441 降到约 244。
- LLM 可见的 hook 行为变化，发布时 minor 版本 + CHANGELOG；关闭开关沿用 `CODE_GRAPH_NO_BLOCK_GREP=1`。
- 复测：`evals/run.sh --tag coding`，有插件组 `recommendations.jsonl` 中 grep 记录应从 0 变为非 0，并比较两组轮数与成本。
