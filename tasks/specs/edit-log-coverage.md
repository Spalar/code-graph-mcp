---
status: approved
revision: 2
---

# 编辑日志覆盖 Write / `sed -i` / `perl -pi`（Q1）

来源：`docs/CLAUDE-USAGE-EVAL-2026-09-28.md` D4 与第 7 节 Q1（本地文档）。2026-09-29 用户批准"按建议执行"：只记日志、不注入。

## goal

Stop 检查（`stop-impact.js`）只对编辑日志里带 `(file, symbol, sigs)` 的记录比较签名。现在只有 Edit 经过编辑前 hook，所以用 Write 覆盖已有文件、用 `sed -i`/`perl -pi` 就地改文件时改掉的签名，Stop 一概看不到（编程评测 3/3 次 subclass 运行用 `sed -i` 修改）。

让这几类编辑也留下 Stop 能用的基线记录，不向模型注入任何内容。

## non-goals

- 不对 Write / `sed -i` 注入影响分析（Write 传的是整个文件，"取最早出现的定义"会给出误导性提示）。
- 不解析 sed/perl 脚本去预测改动；不支持 `find … -exec sed -i`、`xargs sed -i`、通配符操作数。
- NotebookEdit：不加匹配器（见 Change log r2）。
- Stop 的判定逻辑不变（仍是本回合首条记录 vs 工作区）。

## 设计

- `hooks.json` 与 `lifecycle.js` 清单：PreToolUse 编辑前 hook 的匹配器 `Edit` → `Edit|Write`。
- `pre-edit-guide.js`：`tool_name` 为 Write 时记文件级记录；Write 覆盖已有文件时，另对"头部在磁盘版本与 `content` 之间不同"的每个定义记一条带基线的记录；然后退出，不查询、不注入、不写冷却。
- `pre-grep-guide.js`（PreToolUse:Bash）：识别就地编辑命令，对每个操作数文件记文件级记录，并对文件里每个定义记一条基线（无法预先知道哪些会变，交给 Stop 比较；未变的在 Stop 里按"只改了函数体"静默）。
- 上限（hook 预算内）：扫描文件 ≤ 256 KB、每文件定义名 ≤ 64（超过则只记文件级）、每条命令操作数 ≤ 8。

### 接受的命令形态（顶层段、首词即命令）

| 形态 | 记录 |
|---|---|
| `sed -i 's/a/b/' f.py` | f.py |
| `sed -i.bak -e 's/a/b/' -e 's/c/d/' f.py g.py` | f.py、g.py |
| `sed -Ei 's/a/b/' f.py`、`sed --in-place 's/a/b/' f.py`、`sed --in-place=.bak …` | f.py |
| `sed -i -f script.sed f.py` | f.py（`-f` 的值不是操作数） |
| `perl -pi -e 's/a/b/' f.py`、`perl -i -pe …`、`perl -i.bak -pe …` | f.py |
| `git diff; sed -i … f.py`、`cd sub && sed -i … f.py` | f.py（`cd` 后按新目录解析） |
| `sed -ie 's/a/b/' f.py` | f.py（GNU：`-ie` 是备份后缀 `e`，脚本是下一个词） |
| `sed -i '' 's/a/b/' f.py`（BSD 写法） | f.py（按 GNU 读：`''` 是脚本，`'s/a/b/'` 不是已存在的文件） |
| `perl -pie 's/a/b/' f.py` | f.py（perl 按 `-p -i` 扩展名 `e` 读，`'s/a/b/'` 被当作脚本文件而报错；多记的基线在 Stop 里比较后静默） |
| `LC_ALL=C sed -i …`、`sed -i … f.py 2>/dev/null`、`sed -i … f.py > log` | f.py（环境变量前缀、重定向及其目标不算操作数） |

### 不记录的形态（看起来像但不是就地编辑）

| 形态 | 原因 |
|---|---|
| `sed -n 's/a/b/p' f.py`、`sed 's/a/b/' f.py > g.py` | 没有 `-i` |
| `sed -i 's/a/b/' src/*.py` | 通配符：展开结果未知 |
| `echo sed -i x f.py`、`grep 'sed -i' f.py` | 首词不是 sed/perl |
| `perl -pe 's/a/b/' f.py`、`perl -e 'print 1'` | 没有 `-i` |
| 操作数不是项目内的普通文件 | 不存在 / 目录 / 项目外 |

误报的代价是多几条基线记录（Stop 比较后静默），漏报的代价是维持现状，所以语法取窄。

## constraints

- 任何失败都不能影响工具调用本身（hook 失败即放行）。
- `CODE_GRAPH_QUIET_HOOKS=1` 下编辑前 hook 仍记日志（与现在的 Edit 一致）；Bash hook 在该开关下整体不运行，Stop 也被同一开关静默，所以不影响结果。
- 索引构建中也照记（记录不依赖索引）。

## success-criteria

- Write 覆盖一个改了签名的已有 `.py` 文件、调用者在未编辑的文件里：Stop 报告该调用者；同样的改动只改函数体：Stop 静默。
- `sed -i` 把签名改掉：同上；`sed -i` 只改函数体：静默。
- Write 永不向模型注入内容（输出为空）。
- 上表每一行都有测试；每条判定规则有变异被抓。

## open-questions

- 无。

# Change log

- r1（2026-09-29）：初稿，按批准的建议定范围。
- r2（2026-09-29）：去掉 NotebookEdit。`hooks.test.js` 的匹配器面守卫记录了有意排除它的理由（不解析 `.ipynb`，加匹配器要随解析支持一起做）；对 Stop 检查也成立：笔记本没有签名读法，只能记文件级记录，而"本回合碰过"已由 mtime 覆盖。加上它只多一次 node 进程、没有任何效果。
