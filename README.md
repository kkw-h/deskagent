# DeskAgent

给桌面整理器装上语义大脑。

读取桌面，理解每个图标是什么，然后告诉 [PecoFence](https://github.com/DayuanJiang/PecoFence) 该把它们放到哪个栅栏。

## 为什么不做完整的桌面工具

竞品调研后放弃自研桌面应用，理由三条：

1. **该做的有人做了。** PecoFence 用 21 天 10 个版本做到了栅栏、折叠、标签页、多屏、动态壁纸取色。重做是几十人月的重复劳动。
2. **我们缺的那层恰好没人做。** PecoFence 的规则只认扩展名、类型、文件名、大小、时间。它不知道 `Yaak` 是 API 调试工具、`Sciland` 是游戏。
3. **成本集中在最不该自研的部分。** Shell COM、WorkerW、坐标操纵、Direct2D 渲染是苦力活；语义判断才是真有壁垒的部分。

详见 [竞品与技术调研.md](竞品与技术调研.md)。

## 产品边界

| DeskAgent 做 | PecoFence 做 |
|---|---|
| 枚举桌面条目 | 渲染栅栏、玻璃效果 |
| 解析 .lnk 目标路径 / PE 版本信息 / 签名 | 折叠、标签页 |
| 语义分类判断 | 图标提取、缩略图 |
| 把方案编译成 PecoFence 规则 | 拖放、右键菜单、重命名 |
| 调 `pecofence-cli` 落地 | 多显示器布局 |
| 增量监听与归位 | 崩溃恢复 |

PecoFence 未安装时，DeskAgent 仍可独立输出方案 JSON。

## 技术核心：规则编译

PecoFence 的规则引擎是**属性驱动**的，不是语义驱动的。所以分类结果必须拆成两半：

| 分类依据 | 能否变成持久规则 |
|---|---|
| 扩展名 `.pdf` → 文档 | 能 `rule add --ext pdf` |
| 类型 安装包 → 归档 | 能 `rule add --type installers` |
| 文件名 含 Chrome → 浏览器 | 能 `rule add --name-contains` |
| **语义** Steam.lnk → 游戏 | **不能**，只能一次性 `item move` |

这就是本产品存在的全部理由：语义分类无法沉淀成 PecoFence 规则，新装的游戏不会被自动归类。**增量守护必须由 DeskAgent 自己做。**

## 文档

- [PRD.md](PRD.md) — 产品规划 v1.0（转向版）
- [评测报告.md](评测报告.md) — 规则包在三个数据集上的实测结果与局限
- [竞品与技术调研.md](竞品与技术调研.md) — 14 个开源竞品与三条技术路线
- [产品方案.html](产品方案.html) — 可视化方案概览

## 技术栈

Rust CLI。

```
crates/scan       桌面枚举 + .lnk 深解析 + PE 版本信息
crates/classify   规则引擎 + 置信度
crates/compile    分类结果 → 栅栏方案 + 网格布局
crates/pecofence  pecofence-cli 适配层
crates/storage    SQLite（历史、学习规则、缓存，M2 起用）
crates/cli        命令行入口
```

`.lnk` 与 PE 版本信息都是纯 Rust 解析，不依赖 COM，也不需要 `windows` crate。
这样能用 GNU 工具链构建，不必装 3GB 的 MSVC。

## 状态

M0、M1 已完成。当前可用命令：

```bash
deskagent scan                    # 扫桌面，列出条目与元数据覆盖
deskagent classify                # 扫桌面并分类
deskagent plan                    # 生成整理方案，不触碰系统
deskagent apply                   # 打印要执行的动作（dry-run）
deskagent apply --yes             # 真正建栅栏并搬图标
deskagent eval --dir <DIR> --labels <YAML> --split all
deskagent classify-json --input <JSON>   # 分类外部条目（注册表、清单文件）
```

实测（详见 [评测报告.md](评测报告.md)）：

| 数据集 | 覆盖率 | 准确率 |
|---|---|---|
| 桌面（37，调优集） | 97% | — |
| 开始菜单（70，独立来源） | 100%※ | 100%※ |
| 注册表安装程序（53，独立来源） | 96% | 100% |

※ 该数据集已参与调优，数字不可引用。可引用的干净估计是 86%–96%。

| 阶段 | 交付 | 通过条件 | 状态 |
|---|---|---|---|
| M0 | scan + classify，只输出方案 | 难点样本全部正确，准确率 ≥80% | 已完成 |
| M1 | 落地到 PecoFence | 一条命令完成布局，快照可回滚 | 代码完成，待真机验证 |
| M2 | 规则编译 + 增量守护 | 新图标 5 秒内归位建议 | 未开始 |
| M3 | LLM 双模式 + 评测 | 操作数比手写规则低 50% | 未开始 |

**生死线**：AI 分类是否真的比手写规则省事。M0 测出瓶颈在覆盖率（70% → 96%），
但「AI 比手写规则省事」这个对照实验还没做——手写规则目前覆盖 96%，
LLM 的价值只剩下「剩下 4% + 未知软件」。这个判断要等 M3 的数据。

### M1 的三条安全约束

`apply` 会真的改动桌面，所以做了三道防护：

1. **先存快照**。任何修改前调 `snapshot save`，失败即中止。输出里给出回滚命令。
2. **不碰 portal 栅栏**。portal 背后是磁盘上的真实文件夹，往 portal 里 `item move`
   等于在资源管理器里搬文件。同名 portal 会被整组跳过，而不是复用。
3. **`--yes` 才动手**。没有这个标志一律按 dry-run 处理，不依赖 PecoFence 是否安装。

### M1 未验证的部分

`pecofence-cli` 的参数和输出形状是**对源码核对**的，不是实测的——本机没装 PecoFence。
核对过的形状记在 `crates/pecofence/src/lib.rs` 的模块注释里。真机首次运行时需要确认：

- `fence create --rect` 在我们算出的网格坐标上不被 `invalid_value` 拒绝
- `item move` 传完整 `.lnk` 路径能被正确识别为桌面条目
- `monitor list` 的 `workArea` 在非 100% 缩放下取值符合预期

参数拼装全部抽成了纯函数并有断言测试（25 个），因为这部分错了整个产品就错了。

## 历史

v0.3 及之前的完整桌面工具方案见 git 提交 `de2a186`。其中的采集层设计、分类优先级链、隐私规则仍有效，只是执行层整体替换。
