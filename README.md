# PecoFence + DeskAgent

Windows 11 桌面栅栏工具，内置基于软件语义的自动分类。

PecoFence 负责栅栏、玻璃渲染、折叠、标签页、多屏布局；DeskAgent 负责回答「这个图标到底是什么软件」。
两者现在是同一个 workspace，一套构建，一个产品——**分类能力跑在应用进程里**，不再需要另装一个 CLI。
设置 →「整理规则」→「智能整理」先给分组预览，确认后才建栅栏、移图标。

## 这个仓库怎么来的

最初想做独立的桌面整理工具，竞品调研后放弃——PecoFence 用 10 个版本做到了栅栏、折叠、标签页、
多屏、动态壁纸取色，重做是几十人月的重复劳动（见 [竞品与技术调研.md](竞品与技术调研.md)）。

于是先做了个只负责分类的外部工具，通过 `pecofence-cli` 落地。做完整套评测后发现两件事：

1. 瓶颈在**覆盖率**不在准确率——换到没见过的软件，规则覆盖从 97% 掉到 70%。
2. 更关键的：PecoFence 的规则引擎认扩展名、文件名、快捷方式目标、大小、时间，
   **但不认公司名**。而公司名恰恰是判断「这是什么软件」最确定的信号。

所以最终改成把 PecoFence 源码并进来直接改。当前基线是 PecoFence 0.2.3（Apache-2.0）。

## 目录结构

```
crates/core       数据模型 + 规则引擎（纯 Rust，无 Windows 依赖，可跨平台单测）
crates/platform   Win32：Shell COM、命名管道服务端、托盘、热键、文件信息
crates/render     Direct2D / Composition 渲染
crates/app        GUI 主程序（WebView2 设置页）
crates/watchdog   异常退出后恢复桌面图标
crates/ipc        命名管道协议 + 客户端
crates/cli        pecofence-cli（给终端和编码 agent 用）
crates/scan       DeskAgent：.lnk 深解析 + PE 版本信息
crates/classify   DeskAgent：规则包与匹配
crates/compile    DeskAgent：分类结果 → 栅栏方案 / PecoFence 规则
crates/pecofence  DeskAgent：与运行中应用通信的封装（直连 IPC）
crates/llm        DeskAgent：OpenAI 兼容客户端（云端/本地）+ 隐私去标识化
crates/deskagent  DeskAgent 的分析与评测 CLI
tools/bindgen     从 Windows SDK 生成 platform 的 FFI 绑定
vendor/           两个 patch 过的 windows crate（见 Cargo.toml 注释）
```

## 构建

**必须用 MSVC 工具链。** `windows` crate 全家桶在 GNU 下需要 `dlltool` 生成 import lib，
而 rustup 的 `rust-mingw` 组件不提供它——`cargo check` 能过，`cargo test` / `cargo build`
会在链接阶段失败。`.cargo/config.toml` 里已经把 target 固定为 `x86_64-pc-windows-msvc`。

需要：MSVC C++ 工具链（VS 2022 Build Tools 的 `VC.Tools.x86.x64` 工作负载）+ Windows SDK。

```bash
rustup target add x86_64-pc-windows-msvc
cargo build                    # 全部
cargo build -p pecofence       # 只构建 GUI
cargo test --workspace
```

## DeskAgent 融入了什么

不是把两个二进制放进一个仓库，而是三处实质性改动。

### 1. 规则引擎新增公司名条件

`core` 里 `Cond` 多了 `Company { op, value }`，`ItemFacts` 多了 `company` 字段，
`app` 采集条目时从目标可执行文件读 PE 版本信息填充。`Rule::new` 把 `Company` 和
`ShortcutTarget` 一起归入最高优先级的 `Class::Target`——它们都是「这是什么软件」的
确定答案，不该被扩展名规则抢在前面。

```bash
pecofence-cli rule add --name Games --company-contains Valve --to Games
```

顺带补了 `--target-contains`。这个参数**之前根本不存在**，也是我早先误判
「PecoFence 表达不了快捷方式目标路径」的原因：核心的 `Cond::ShortcutTarget` 一直有，
只是 CLI 没暴露。

### 2. 规则编译产出原生条件

`compile::rules::compile_conds` 把 DeskAgent 的规则包（按产品族组织、含公司名）编译成
`Vec<Cond>`，直接走 `rules.add`。四种分类信号现在全部可表达：

| 分类依据 | PecoFence 条件 |
|---|---|
| `ext` | `Cond::Ext` |
| `name_contains` | `Cond::Name` |
| `target_contains` | `Cond::ShortcutTarget` |
| `company` | `Cond::Company`（本轮新增） |

### 3. 直连 IPC，不再起子进程

`crates/pecofence` 的对外 API 没变，内部实现从「起 `pecofence-cli`、拼命令行、解析 stdout」
换成直接调 `pecofence_ipc::client`。好处：不用在 PATH 里找可执行文件、不存在版本不一致、
参数是 `Method` 枚举（拼错编译期就暴露）、少一次进程启动。

为此把 `client.rs` 从 `crates/cli` 移进了 `crates/ipc`——IPC 客户端本来就该和协议在一起。

### 4. 语义分类搬进应用本体

前三步只是打通了数据和协议，判断本身还留在外部 CLI 里。现在它也在应用里了：

| 位置 | 做什么 |
|---|---|
| `crates/app/src/semantics.rs` | 内置规则包 `include_str!` 编进来；桌面条目 → 分类记录 → 分组（纯函数，有单测） |
| `crates/app/src/app/semantics.rs` | 预览、落地（先存快照）、按标题复用已有栅栏、在空位新建 |
| IPC `semantics.plan` / `semantics.apply` | 预览只读；落地会建栅栏、移条目，返回逐组结果 |
| 设置页「智能整理」 | 分组预览 → 确认并整理；规则编辑器新增「公司名包含」条件 |
| 新条目自动归类 | 「新项目出现时自动归类」打开时，属性规则先判，规则表达不了的交给语义 |

两条边界是刻意留的：

- **规则优先。** 扩展名、类型这些属性规则认得出的，语义不再插手——与对照实验的结论一致
  （文档类规则又快又免费，AI 没有优势）。
- **一个条目不新建栅栏。** 它能进**已存在**的同名栅栏；要新建栅栏至少两个条目，
  否则每拖进一个文件就长出一个只有一个图标的栅栏。

`deskagent` CLI 仍然保留：它做评测（`eval`/`bench`）、批量分析，以及没有 GUI 的场景。

## 命令

```bash
# 产品 CLI（输出 JSON，面向终端和编码 agent）
pecofence-cli describe            # 命令目录
pecofence-cli skill               # 给编码 agent 的使用指南
pecofence-cli status
pecofence-cli fence list
pecofence-cli rule add --name PDFs --ext pdf --to Docs
pecofence-cli rule add --name Games --company-contains Valve --to Games

# DeskAgent（输出人可读文本，面向分析与评测）
deskagent classify                # 扫描桌面并分类
deskagent plan [--llm]            # 生成方案，不触碰系统
deskagent apply [--yes]           # 落地到 PecoFence（GUI 里也有同样的入口）
deskagent watch [--once]          # 增量守护
deskagent eval --labels <YAML>    # 用带标注的样本评测规则包
deskagent bench --labels <YAML>   # 对照实验：规则 vs 模型的人工操作成本
```

## 实测数字

规则包（详见 [评测报告.md](评测报告.md)）：

| 数据集 | 覆盖率 | 准确率 |
|---|---|---|
| 桌面（37，调优集） | 97% | — |
| 开始菜单（70，独立来源） | 100%※ | 100%※ |
| 注册表安装程序（53，独立来源） | 96% | 100% |

※ 该数据集已参与调优，数字不可引用。可引用的干净估计是 86%–96%。

对照实验（人工操作次数，越少越好）：

| 样本 | 手写规则 | 模型（96% 准确率） | 结论 |
|---|---|---|---|
| 70 条快捷方式 | 70 | 3.8 | 模型大幅胜出 |
| 4 条文档/媒体 | 2 | 1.4 | 规则胜出，模型最多打平 |

这条决定了产品定位：AI 的价值不在「比规则更准」，而在「规则写起来太贵的场景」。
扩展名能表达的（文档、图片、音视频），规则又快又免费；几十个软件各要一条语义规则时，
模型才划算。

> 注意：这个对照实验是在「PecoFence 表达不了 target_path」的前提下做的。
> 该前提后来被证伪（`Cond::ShortcutTarget` 一直存在），所以模型侧的领先幅度
> 没有表格里那么大。真正只有 `company` 是 PecoFence 原本没有的——现在也补上了。
> 数字保留原样，结论需要按这个修正理解。

## 安全约束

`deskagent apply` 会真的改动桌面：

1. **先存快照**。任何修改前调 `snapshot save`，失败即中止，输出里给回滚命令。
2. **不碰 portal 栅栏**。portal 背后是磁盘上的真实文件夹，往里 `item move`
   等于在资源管理器里搬文件。同名 portal 整组跳过，不复用。
3. **`--yes` 才动手**。没有这个标志一律 dry-run，且 dry-run 不依赖 PecoFence 是否运行。

模型侧另有两道：只上传去标识化字段（路径只保留产品段，用户名和盘符不出去）；
模型发明的分组名一律丢弃，宁可判不出来也不去创建不存在的栅栏。

## 已知限制

- **降级不兼容**：含 `company` 条件的配置在 0.2.3 之前的版本里读不了（unknown variant）。
  降级前需要先删掉这类规则。
- **模型侧数字是假设值**。`bench` 的 96% 是 `--assume-accuracy` 代入的算术结果。
  接真实模型跑一次不传该参数才知道真相。
- `crates/storage` 尚未创建，历史命中率和用户纠正学习没做。
- LLM 只验证过 HTTP 链路与解析（本地假服务），没接真实模型跑过。

## 文档

- [PRD.md](PRD.md) — 产品规划与实施结果回填
- [评测报告.md](评测报告.md) — 规则包在三个数据集上的实测结果与局限
- [竞品与技术调研.md](竞品与技术调研.md) — 14 个开源竞品与三条技术路线
- [产品方案.html](产品方案.html) — 可视化方案概览
- [docs/工程备忘.md](docs/工程备忘.md) — 本机环境事实、已修的坑、方法论教训
- [docs/](docs/) — PecoFence 原项目文档（CLI 参考、架构说明）

## 许可证

Apache-2.0。包含 PecoFence 代码，保留其 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)。
第三方组件许可见 [third_party/README.md](third_party/README.md)。
