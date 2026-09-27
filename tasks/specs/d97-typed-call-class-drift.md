---
status: implemented
revision: 3
---

# D#97 M-N3 — 类结构变化后，未改动文件里的 typed 调用要按 rebuild 重解析

## 目标

一次增量运行如果改变了项目的类结构，其结果必须与同一棵树的 rebuild 一致。

## 缺陷

`recv_type_targets` 的输入是全局的类结构：
- 哪些类的最后一段名字是 `T`（是否已知、是否唯一、是否嵌套）；
- 谁继承谁（`inherits` 边决定 override）；
- 这些类里谁定义了该方法。

只要其中任意一项变化，没被改动的调用方在 rebuild 时就会绑得不一样。而增量只会重解析两类调用：一是本次运行涉及文件里的调用，二是指向被改文件、恢复失败后重新入队的调用。实测差异：删除 django 的 `forms/fields.py` 后差 19 条边，删除 leveldb 的 `iterator.h` 后差 861 条边。

## 接受的形态（语料：`src/indexer/pipeline/tests.rs`，6 个测试，实现前均为 RED）

| 形态 | 测试 |
|---|---|
| 删掉两个同名类中的一个 → 剩下的变唯一 → 开始追 override | `test_class_delete_re_resolves_untouched_typed_callers` |
| 改名离开 / 改名进入同名 | `test_class_rename_re_resolves_untouched_typed_callers` |
| 新文件新增 override 子类 | `test_new_subclass_override_reaches_untouched_typed_callers` |
| 已有子类新增 override 方法 | `test_new_override_method_reaches_untouched_typed_callers` |
| 子类换基类 → override 关系消失 | `test_changed_base_drops_override_from_untouched_typed_callers` |
| C++：删掉顶层 `Iterator` → 嵌套的 `SkipList::Iterator` 开始回答裸名 `Iterator` | `test_top_level_class_delete_re_resolves_untouched_cpp_callers` |

## 设计

typed 调用只在 deferred pass 一处解析。它的输入变了，就把调用方文件交给 D#24 已有的第二轮重抽取（`fan_out_to_new_duplicate_definitions`）。不走 pending 重新入队，原因有四：
1. pending 的唯一键会吞掉同一函数里、同名但类型不同的第二条调用（django 39 对）；
2. pending 行 50 次后会被清掉；
3. 只有删除操作的运行不跑 sweep；
4. sweep 和 deferred 是两套实现，需要一直保持一致。

**漂移判定**：沿用 `scope_names_from_count_drift` 的做法，比较运行前后的快照，而不是边跑边记账。
在 `snapshot_definition_counts` 里，对本次运行涉及的路径额外记录一份与行号无关的类结构集合：
- `Class(path, lang, COALESCE(qualified_name, name), nested)`
- `Inherits(path, sub.name, base.name)`
- `Method(path, qualified_name)`：只记 qualified 里带 `.` 的方法

前后两份集合的对称差给出：
- 类名种子 = Class 行的最后一段 ∪ Inherits 行的 sub 和 base；
- 方法对 = Method 行的（owner 最后一段, 方法名）。

然后在当前 `inherits` 图上按名字求祖先闭包：
- `D_any = closure(类名种子)`
- `D_method = {(n, m) | (O, m) ∈ 方法对, n ∈ closure({O})}`

**依赖方**：从 `q ∈ {rtype, super}` 的 `calls` 边（含 `amb`）和 pending 行中，取满足下面任一条件的调用方文件，去掉本次运行已经涉及的路径：
- `v` 的最后一段 ∈ `D_any`；或
- （最后一段, 目标名）∈ `D_method`。

## 成本（django 实测，见 memory `project_d97_typed_receiver_leftovers_measured`）

- 只在类结构有漂移的运行里查依赖方：全表扫 typed 边约 30 ms。
- 每重抽取一个文件约 9 ms；依赖文件数 p90 为 13–27，最多 92。
- 普通改行只多两次按路径的快照，需实测无操作增量和单文件编辑的耗时不回退。

## 修订 2：闭包到达的名字要求唯一

实测（django，在 `forms/fields.py` 末尾追加一个 `Field` 子类的 `clean` override）：初版多花 1.1 s，但边集合在基线里本来就没有差异。原因是 `recv_type_targets` 只对名字唯一的类追 override，而 django 的 `Field` 不唯一。
因此，经祖先闭包到达的名字只保留在当前索引中唯一的那些。移动了的类（`c` 行）以及方法的 owner 本身不做过滤。这是精确的：名字的类节点数量变了，必然有同名类节点在本次运行里增删，这时该名字作为 `c` 行种子直接纳入。

## 变异矩阵（14 个用例，每个变异都被预期断言杀死）

| 变异 | 杀死它的用例 |
|---|---|
| 整体停用 | 全部 12 个漂移用例 |
| `any` 无祖先闭包 | grandchild_changed_base |
| 方法无祖先闭包 | new_override_method、moving_code（正向对照） |
| 忽略 `c` / `i` / `m` 行 | methodless_delete / changed_base、grandchild_changed_base / new_override_method、free_function_turned_method |
| 关闭唯一性过滤 | override_of_a_shared_class_name（效率守卫） |
| 不并入移动的类本身 | methodless（改名成同名） |
| owner 本身也要求唯一 | ~~free_function_turned_method~~：发布前评审在 fbed158 上复测，未被杀死（见下） |

发布前评审（2026-09-27）另做的变异中，下列会改变行为，但全套测试都通过，目前没有测试钉住：
- `per_method` 要求 owner 本身唯一（上表最后一行）；
- `inherited_or_overridden` 追 override 时不要求类名唯一；
- 链改写去掉"项目类"判断；
- 漂移的 `field_keys` 不含 `linked`（基类边变动）。

已补测试钉住的有：两个基类都定义方法时判为 ambiguous；注解后跟 `override` / `final` / `noexcept`。

## 实测

| 场景 | 基线（只在 rebuild / 只在增量） | 新版本 | 增量耗时 基线 → 新 |
|---|---|---|---|
| django 删除 `forms/fields.py` | 19 / 0 | 0 / 0 | 4,560 → 6,908 ms |
| django 改名 `forms.CharField` | 5 / 0 | 0 / 0 | 978 → 3,168 ms |
| django 追加 `Field` 的 override | 0 / 0 | 0 / 0 | 3,652 → 3,564 ms |
| leveldb 删除 `iterator.h` | 861 / 457 | 0 / 0 | 20 → 392 ms |
| leveldb 删除 `skiplist.h` | 3 / 3 | 0 / 0 | 257 → 648 ms |

普通编辑（在同一索引上交替跑两个二进制，各 9 次取中位）：单文件编辑 1,209 → 1,246 ms（区间重叠），编辑类文件但不改结构 885 → 860 ms，无操作 102 → 102 ms。

## 不覆盖

- 不留痕迹的 typed 调用。噪声名 Fallback（`x.build()`）原先会被直接丢弃，后来改为进入 pending 缓冲（django 增加 42 行，原有 29,075 行）。现在只剩一种情况：pending 行经过 50 次解析运行后被清掉，之后类或方法才出现，这时需要 `rebuild-index`。要彻底覆盖，需要持久化的（文件, 类名）依赖表。
