# DeskAgent 产品规划 v1.0（转向版）

> 2026-10-28 ｜ 产品形态：CLI 工具 ｜ 定位：给桌面整理器装上语义大脑

---

## 一、为什么转向

v0.3 及之前规划的是一个完整的桌面整理工具：自绘收纳盒、操纵 Shell 图标坐标、MCP 接入。竞品调研后放弃，理由三条：

**① 该做的事已经有人做了，而且做得很好。**
PecoFence（Rust, Apache-2.0, 686★）用 21 天 10 个版本做到了栅栏、折叠、标签页、多屏、动态壁纸取色、原生右键菜单、崩溃恢复。DeskBox（7.7k★）把原生质感做到极致。重做这些是几十人月的重复劳动。

**② 我们真正缺的那一层，恰好是它们没做的。**
PecoFence 的规则只认**扩展名、类型、文件名模式、大小、创建时间**。它不知道 `Yaak` 是 API 调试工具、`Sciland` 是游戏、`Zed` 是代码编辑器。用户得自己写 `--ext pdf`、`--name-contains Chrome`。

**③ 自研的成本集中在最不值得自研的部分。**
Shell COM、WorkerW 挂载、坐标操纵、watchdog、Direct2D 渲染——这些是苦力活，且已有成熟实现。而"判断这个图标属于哪"才是真正有壁垒、有差异的部分。

**结论：不做桌面工具，只做分类器。** 用 PecoFence 落地，我们负责它做不了的事。

---

## 二、产品定义

**DeskAgent 读取你的桌面，理解每个图标是什么，然后告诉 PecoFence 该把它们放到哪个栅栏。**

它不渲染、不折叠、不管多屏——那些交给 PecoFence。它只做一件事：**把语义判断变成可执行的分类决策**。

```
桌面文件/快捷方式
      ↓  DeskAgent 采集：解析 .lnk 目标路径、PE 版本信息、数字签名
      ↓  DeskAgent 分类：规则引擎 + LLM，判断每个条目属于哪
      ↓  DeskAgent 落地：调 pecofence-cli 建栅栏、加规则、移动图标
PecoFence 栅栏（用户看到的最终效果）
```

---

## 三、产品边界

| 我们做 | 我们不做（交给 PecoFence） |
|---|---|
| 枚举桌面条目 | 渲染栅栏、玻璃效果 |
| 解析快捷方式（目标路径 / PE 信息 / 签名） | 折叠、标签页 |
| 语义分类判断 | 图标提取、缩略图 |
| 生成分类方案 | 拖放、右键菜单、重命名 |
| 把方案编译成 PecoFence 规则 | 多显示器布局 |
| 调 `pecofence-cli` 落地 | 崩溃恢复、watchdog |
| 增量监听与归位 | 图标坐标操纵 |

**PecoFence 未安装时**：DeskAgent 仍可独立运行，只输出分类方案 JSON，不落地。工具本身不依赖 PecoFence 存在。

---

## 四、核心工作流

```
scan → classify → plan → [人工审阅] → apply → verify
                  ↓
              compile: 可规则化的 → rule add（持久）
                       不可规则化的 → item move（一次性）
```

**关键设计：分类结果拆成两半。**

PecoFence 的规则引擎是**属性驱动**的（扩展名/类型/名字/大小/时间），不是**语义驱动**的。所以：

| 分类类型 | 例子 | 能否变成持久规则 |
|---|---|---|
| 扩展名可判 | `.pdf/.docx` → 文档 | ✅ `rule add --ext pdf,docx` |
| 类型可判 | 所有安装包 → 归档 | ✅ `rule add --type installers` |
| 名字可判 | 名字含 `Chrome` → 浏览器 | ✅ `rule add --name-contains` |
| **语义可判** | `Steam.lnk` 目标是 `\Steam\steam.exe` → 游戏 | ❌ **规则表达不了** |

**语义类的分类无法沉淀成 PecoFence 规则**，只能一次性 `item move`。这意味着新装的游戏不会被自动归类——**所以增量守护必须由 DeskAgent 自己做**，不能指望 PecoFence 的规则。

这是本产品存在的核心理由，也是它比"手写规则"多出来的全部价值。

---

## 五、模块设计

### 5.1 采集

**桌面枚举**
- 路径从注册表 `User Shell Folders\Desktop` 读（OneDrive 重定向会变）
- 同时扫 `%PUBLIC%\Desktop`
- 过滤 `desktop.ini`、隐藏项、系统项

**快捷方式深解析**（核心能力）

`.lnk` 要挖出四层：

| 层级 | 字段 | 来源 |
|---|---|---|
| 1 | 目标路径 / 参数 / 工作目录 | `IShellLink` 或 LNK 二进制解析 |
| 2 | 公司名 / 产品名 / 文件描述 / 版本 | PE VersionInfo 资源 |
| 3 | 数字签名主体 | `WinVerifyTrust` + `CryptQueryObject` |
| 4 | 创建 / 修改时间 | 文件系统 |

`.url` 走 INI 解析（只取 host，剥离查询参数）。

**为什么要挖这么深**：`Zed`、`Yaak`、`Sciland`、`夸克` 这类名字毫无信息量的条目，只有目标路径和 PE 信息能救。这是分类准确率的地基。

**不提取图标。** v0.3 打算用图标做视觉识别，但 `GetImage` 默认返回缩略图（会泄露文档内容），且视觉模型成本高、收益不确定。先用元数据跑通，图标识别留作可选增强。

### 5.2 分类

**五层优先级链**（保留 v0.3 设计）

```
1. 用户明确指定 / 锁定
2. 用户手写规则
3. 用户已确认的学习规则
4. 内置规则包
5. LLM 建议
```

前四层本地完成，零延迟零成本。只有第 5 层调模型，且只处理前四层未命中的条目。

**内置规则包**（离线，覆盖常见软件）

```yaml
- match: { company: "Valve Corporation" }
  group: 游戏
- match: { target_contains: "\\Steam\\" }
  group: 游戏
- match: { target_contains: "\\Tencent\\WeGame\\" }
  group: 游戏
- match: { company: "JetBrains" }
  group: 开发
- match: { ext: [".psd", ".ai", ".sketch", ".fig"] }
  group: 设计
```

规则引擎匹配的是**富元数据**（company / target_path），这是 PecoFence 规则做不到的。

**LLM 接入（双模式）**

| 模式 | 用途 | 默认 |
|---|---|---|
| 本地 Ollama | 隐私敏感、离线可用 | ✅ 优先 |
| 云端 OpenAI 兼容 | 准确率更高 | 可选 |

只发送白名单字段（见 §8），批量提交，一次请求处理所有低置信度条目。

**置信度**：用历史接受率的 Wilson 下界，不用模型自报概率。冷启动阶段全部进"建议区"，由用户确认，确认过程本身在积累数据。

### 5.3 规则编译（技术核心）

把分类结果编译成 PecoFence 能执行的指令。分两步：

**第一步：判断每个分组能否规则化**

对同一分组内的所有条目，检查是否有共同的可表达特征：

```rust
fn compilable(group: &Group) -> Option<Rule> {
    // 1. 全部同扩展名 → --ext
    if group.items.iter().map(|i| i.ext).unique().count() == 1 {
        return Some(Rule::ext(group.ext));
    }
    // 2. 全部同类型 → --type
    if group.items.iter().map(|i| i.kind).unique().count() == 1 {
        return Some(Rule::kind(group.kind));
    }
    // 3. 全部共享名字前缀 → --starts-with / --name-contains
    if let Some(prefix) = common_prefix(&group.names) {
        if prefix.len() >= 3 { return Some(Rule::name_contains(prefix)); }
    }
    // 4. 语义分类（靠 company / target_path 判出来的）→ 不可规则化
    None
}
```

**第二步：不可规则化的走一次性移动**

```bash
pecofence-cli item move --glob "Steam.lnk" --to 游戏 --dry-run
pecofence-cli item move --glob "WeGame.lnk" --to 游戏 --dry-run
```

注意 `--glob` 必须配 `--from`（PecoFence 文档建议），避免误伤 portal 栅栏里的文件。

**输出示例**

```json
{
  "groups": [
    { "name": "游戏", "fence": "游戏", "items": 9, "rule": null, "mode": "move" },
    { "name": "文档", "fence": "文档", "items": 14, "rule": {"ext": ["pdf","docx"]}, "mode": "rule" }
  ]
}
```

### 5.4 落地执行

**调 PecoFence CLI 的顺序**（严格遵守，保证可回滚）

```bash
# 1. 先快照
pecofence-cli snapshot save "deskagent-before-apply"

# 2. 确认现有栅栏，避免重复创建
pecofence-cli fence list

# 3. 创建缺失的栅栏
pecofence-cli fence create --title 游戏

# 4. 加规则（可规则化的部分）
pecofence-cli rule add --name "文档" --ext pdf,docx --to 文档

# 5. 移动（不可规则化的部分），先 dry-run
pecofence-cli item move --glob "Steam.lnk" --from 桌面 --to 游戏 --dry-run

# 6. 真正执行
pecofence-cli item move --glob "Steam.lnk" --from 桌面 --to 游戏
```

**错误处理**：PecoFence CLI 退出码语义——0 成功 / 1 应用错误或部分失败 / 2 用法错误 / 3 应用未运行 / 4 超时。

超时（4）时**不重试**，先 `fence list` 读回状态再决定（PecoFence 文档明确要求，因为请求可能已生效）。

### 5.5 增量守护

PecoFence 的规则覆盖不到语义分类，所以新图标归位由 DeskAgent 负责。

```
监听 Desktop 目录（notify crate）
  → 新条目出现
  → 只对这个条目跑分类
  → 命中内置规则或高置信度 → 调 item move
  → 低置信度 → 托盘/终端提示，等用户确认
```

**为什么不用 PecoFence 的 `watch`**：它的 `watch --once` 是事件触发器，只能告诉我们"有文件落地"，不提供语义判断。判断还是得我们做。

---

## 六、CLI 设计

沿用 PecoFence 的自描述模式（`describe` / `skill`），让 AI agent 能自己发现能力。

```bash
deskagent scan [--json]              # 扫描桌面，输出条目清单
deskagent classify [--dry-run]       # 分类，输出方案
deskagent plan --out plan.json       # 生成可审阅的方案文件
deskagent apply plan.json            # 落地（调 pecofence-cli）
deskagent apply plan.json --dry-run  # 只打印将执行的命令
deskagent rules export               # 导出可规则化部分为 rules.json
deskagent watch                      # 增量守护
deskagent eval --report eval.md      # 对照实验评测
deskagent describe                   # 命令目录 + JSON Schema
deskagent skill                      # 给 AI agent 的操作指南
```

**输出约定**（学 PecoFence）
- stdout 只出 JSON，错误走 stderr
- 退出码：0 成功 / 1 部分失败 / 2 用法错误 / 3 PecoFence 未运行 / 4 超时
- 所有写操作支持 `--dry-run`

**为什么是 CLI 不是 MCP**：编码 agent（Claude Code / Cursor / Codex）都有 Bash 工具，能直接跑 CLI；MCP 需要客户端配置。CLI 还能被脚本和测试直接驱动。后续如需 MCP，只需把 CLI 命令映射成工具，不用重写。

---

## 七、分类评测

**这是项目的生死线。** 如果 AI 分类不比手写规则省事，整个产品没有存在意义。

### 7.1 对照实验

| 项 | 设计 |
|---|---|
| 样本 | 真实桌面 ≥ 40 条目，含快捷方式 / 文档 / 截图 / 临时文件 |
| A 组 | 用户用 PecoFence 原生功能手写规则 |
| B 组 | DeskAgent 自动分类 |
| 指标 1 | 达到可用布局所需的用户操作数（点击/输入次数） |
| 指标 2 | 总耗时 |
| 指标 3 | 24 小时后找文件的速度 |
| 判定 | B 的操作数比 A 低 **50% 以上**，且分类准确率 ≥ 80% |

判定线比 v0.3 更严（50% vs 30%），因为现在对照组是成熟产品，优势必须明显才值得做。

### 7.2 准确率评测

| 指标 | 目标 |
|---|---|
| 顶层分类准确率 | ≥ 85% |
| 需人工修正的条目占比 | < 15% |
| 低置信度正确进入"建议区"的比例 | ≥ 90% |

**样本集**：≥ 30 快捷方式 / 15 文档 / 15 图片 / 10 临时文件 / 5 文件夹。留 30% 不参与调优。

**当前桌面只有 35 个快捷方式，不满足评测要求**，需先补充样本。

---

## 八、隐私

| 字段 | 是否上传 | 处理 |
|---|---|---|
| 完整路径 | ❌ 永不 | 仅本地用于规则匹配 |
| 文件名 | 条件 | 默认脱敏，开关打开才原文发送 |
| 快捷方式参数 | ❌ 永不 | 可能含令牌、账号 |
| URL 查询参数 | ❌ 剥离 | 只留 scheme + host + path |
| 公司名 / 产品名 | ✅ | PE 版本信息，明文 |
| 数字签名主体 | ✅ | 明文 |
| 文件大小 / 时间 | ✅ | 明文 |

**不提取图标**，因此不存在缩略图泄露问题（v0.3 的 `SIIGBF_ICONONLY` 问题在本方案中自动消失）。

**敏感项排除覆盖上传阶段**：文件名含"密码/身份证/简历/合同/invoice/passport"等，直接跳过分类，不进 LLM 请求。

**离线失败不自动上云**：本地模型不可用时提示失败，不静默切换。

---

## 九、技术方案

### 9.1 项目结构

```
deskagent/
├── crates/
│   ├── scan/       桌面枚举 + .lnk 深解析 + PE 版本信息
│   ├── classify/   规则引擎 + LLM 客户端 + 置信度
│   ├── compile/    分类结果 → PecoFence 指令
│   ├── pecofence/  pecofence-cli 适配层（子进程调用）
│   ├── storage/    SQLite（历史、学习规则、缓存）
│   └── cli/        命令行入口
└── rules/          内置规则包 YAML
```

比 v0.3 少了 shell-win / render / app 三个 crate，多了 compile / pecofence。

### 9.2 依赖

```toml
[workspace.dependencies]
windows = { version = "0.58", features = [
  "Win32_Storage_FileSystem",      # PE 版本信息
  "Win32_Security_WinTrust",       # 签名验证
  "Win32_System_Registry",         # 桌面路径
  "Win32_Foundation",
] }
lnk = "0.5"                        # 纯 Rust 解析 .lnk，不依赖 COM
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
rusqlite = { version = "0.32", features = ["bundled"] }
notify = "6"
reqwest = { version = "0.12", features = ["json", "blocking"] }
anyhow = "1"
thiserror = "2"
tracing = "0.1"
```

**不需要 `Win32_UI_Shell` 的 COM 部分**——`.lnk` 用纯 Rust 解析，省掉整套 COM 样板代码。

### 9.3 性能目标

| 阶段 | 目标（40 条目） |
|---|---|
| 桌面枚举 | < 200ms |
| .lnk 深解析（含 PE 版本信息） | < 1.5s（并行 + 缓存） |
| 规则分类 | < 50ms |
| LLM 分类（低置信度条目） | < 8s 云端 / < 15s 本地 |
| **端到端（无 LLM）** | **< 2s** |

---

## 十、路线图

比 v0.3 短得多，因为没有渲染和交互。

### M0 · 分类准确率验证（1 周）

**交付**
- `deskagent scan`：桌面枚举 + .lnk 深解析
- `deskagent classify --dry-run`：分类，输出方案 JSON（不落地）
- 内置规则包（30 条）

**通过条件**
| # | 验收项 | 标准 |
|---|---|---|
| 1 | 深解析覆盖率 | 35 个快捷方式中 ≥ 33 个能拿到目标路径 |
| 2 | PE 信息覆盖 | ≥ 28 个能拿到公司名或产品名 |
| 3 | 规则分类准确率 | 对已知样本 ≥ 80% |
| 4 | 端到端耗时 | < 2s |
| 5 | 难点样本 | `Zed` / `Yaak` / `Sciland` / `夸克` 四个全部正确归类 |

**第 5 项不通过**：说明元数据不够，需引入图标视觉识别，重新评估成本。

### M1 · 落地到 PecoFence（1 周）

**交付**
- `pecofence` 适配层
- `deskagent apply`：建栅栏 + 加规则 + 移动图标
- 快照先行、dry-run、错误处理

**通过条件**
| # | 验收项 | 标准 |
|---|---|---|
| 1 | 端到端可用 | 一条命令完成从扫描到 PecoFence 布局 |
| 2 | 可回滚 | 执行后 `snapshot restore` 能完整还原 |
| 3 | 幂等 | 重复执行不重复建栅栏、不重复加规则 |
| 4 | PecoFence 未运行 | 退出码 3，给出清晰提示，不崩溃 |

### M2 · 规则编译与增量守护（2 周）

**交付**
- 规则编译器（扩展名/类型/名字 → PecoFence 规则）
- `deskagent watch`：增量监听与归位
- 学习规则（用户修正沉淀）

**通过条件**
| # | 验收项 | 标准 |
|---|---|---|
| 1 | 规则编译正确性 | 编译出的规则在 PecoFence 里 `rule apply --dry-run` 结果与预期一致 |
| 2 | 增量延迟 | 新图标出现后 5 秒内给出归位建议 |
| 3 | 不误伤 | 不移动 portal 栅栏内的文件 |
| 4 | 学习生效 | 同类修正 3 次后提出规则建议 |

### M3 · LLM 与评测（2 周）

**交付**
- LLM 双模式接入
- 对照实验报告
- `deskagent describe` / `skill` 自描述

**通过条件**
| # | 验收项 | 标准 |
|---|---|---|
| 1 | 分类准确率 | ≥ 85% |
| 2 | 对照实验 | 操作数比手写规则低 ≥ 50% |
| 3 | 隐私审计 | 抓包验证无完整路径、无参数外传 |
| 4 | 离线可用 | 断网 + 本地模型核心流程完整 |

**总工期约 6 周**（单人，含验证）。对比 v0.3 的 13–15 周，省下的全是渲染和 Shell 交互。

---

## 十一、风险

| 风险 | 影响 | 应对 |
|---|---|---|
| AI 分类准确率上不去 | 产品无价值 | M0 先验证，不达标就停 |
| PecoFence CLI 变更 | 适配层失效 | 固定版本范围，检测 `describe` 输出变化 |
| PecoFence 项目停止维护 | 依赖断裂 | DeskAgent 可独立输出方案 JSON，不绑死 |
| 用户不愿装两个工具 | 采用率低 | 提供"只输出方案"模式，用户手动落地 |
| 语义分类无法规则化 | 增量体验差 | 由 DeskAgent 自己做守护，不依赖 PecoFence |
| 元数据不足以分类 | 准确率低 | 引入图标识别（成本上升，M0 后评估） |

---

## 十二、与 v0.3 的差异摘要

| 维度 | v0.3 | v1.0 |
|---|---|---|
| 产品形态 | 完整桌面应用 | CLI 工具 |
| 渲染 | 自己做（WorkerW + 覆盖层） | 不做，用 PecoFence |
| 图标操纵 | `IFolderView2` 改坐标 | 不做，用 `pecofence-cli item move` |
| 折叠 | 自己实现（判定不可行） | 不做，PecoFence 已有 |
| 接入方式 | MCP 为主 | CLI 为主 |
| 核心模块 | 采集/分类/布局/执行/接入 | 采集/分类/**编译**/适配 |
| 工期 | 13–15 周 | **约 6 周** |
| 风险 | 高（渲染管线） | 中（只赌分类准确率） |

---

## 十三、实施结果（计划完成后回填）

四个里程碑全部代码完成。详细数据见 [评测报告.md](评测报告.md)，这里只记与计划的偏差。

### 验收结果

| 阶段 | 计划通过条件 | 实际 | 结论 |
|---|---|---|---|
| M0 | 目标路径覆盖 ≥33/35 | 37/37 = 100% | 达标 |
| M0 | 公司名覆盖 ≥28 | 29/37 | 达标 |
| M0 | 规则准确率 ≥80% | 独立样本 96% | 达标 |
| M0 | 端到端 <2s | <0.1s | 达标 |
| M0 | 四个难点样本全对 | 全对 | 达标 |
| M1 | 一条命令完成布局，快照可回滚 | 代码完成 | 待真机验证 |
| M2 | 新图标 5 秒内归位建议 | 代码完成 | 待真机验证 |
| M3 | 操作数比手写规则低 50% | 快捷方式样本低 95%，文档样本未达标 | **分场景成立** |

### 三处必须修正的计划假设

**1. 「AI 处理低置信度条目」的定位是错的。**

计划假设瓶颈在准确率。实测瓶颈在**覆盖率**：桌面 97% 是调优出来的假象，
换到 70 个没见过的软件上只有 70%。补完规则包后覆盖率达到 96%，
但这也意味着「AI 兜底低置信度」几乎没有活可干。

**2. 对照实验的初始口径算错了，修正后结论反转。**

第一版按「规则条数」算人工成本：70 个条目 12 条规则就覆盖了，AI 看上去很难赢。
这个口径错在假设这些规则用户写得出来——PecoFence 的规则条件里没有公司名、
没有快捷方式目标路径，而快捷方式的分类全靠这两样。对快捷方式来说规则不是
「写得累」，是**根本写不出来**。

修正后：快捷方式为主的样本，规则侧 70 次操作 vs 模型侧 3.8 次，模型大幅胜出；
文档/媒体为主的样本，规则侧 2 次 vs 模型侧 1.4 次，**模型未达标**。

**3. 产品定位需要改写。**

原表述「给桌面整理器装上语义大脑」暗示我们在判断能力上更强。
实际我们补的是**表达能力缺口**：扩展名能表达的分类，PecoFence 规则又快又免费，
AI 没有优势；规则表达不了的分类，AI 是唯一选项。

准确的定位：**DeskAgent 补的是 PecoFence 规则引擎的表达能力缺口，不是判断能力缺口。**

### 计划外新增

- **评测体系**：三个数据集（桌面/开始菜单/注册表安装程序）+ `eval` 命令，
  支持 dev/test 切分且 test 切分隐藏逐条错误，防止拿测试集调规则
- **隐私去标识化**：上传给模型的路径压成「产品目录 + 文件名」两段，
  用户名和盘符不出去（计划里只说「路径不上传」，太粗，实际做不到就不叫隐私保护）
- **`describe` / `skill` 自描述**：计划放在 M3，实际提前做了，因为编码 agent 需要它
- **普通文件采集**：原实现只收 `.lnk`/`.url`，普通文件被跳过。
  真实桌面一定有 PDF 和图片，这是计划里的遗漏

### 仍然未验证的

- 真实模型分类准确率。`bench` 的模型侧数字是 `--assume-accuracy` 代入的算术结果
- 多显示器、非 100% 缩放下的栅栏布局

PecoFence 真机集成已经在 §14 补完并实测。

---

## 十四、v1.1：语义能力并入应用本体

§一至§十三描述的是「分类器 + 外部 CLI」的形态。合并 PecoFence 源码之后又走了一步：
**判断本身也搬进了应用**，`deskagent` 从「唯一入口」变成「评测与脚本入口」。

### 为什么还要再走一步

合并后 PecoFence 能执行 `company` / `shortcutTarget` 条件，但**没有任何路径从应用内部发起判断**：
用户仍要开终端跑 `deskagent apply`，GUI 里一个入口都没有。能力在，产品不在。

### 现在的形状

```
PecoFence 应用进程
  ├── 采集：shell 枚举 + .lnk 解析 + PE 版本信息（scan）
  ├── 属性规则：扩展名 / 类型 / 名字 / 目标 / 公司名（core::rules）
  ├── 语义规则：内置规则包 classify → 分组（app::semantics）
  └── 落地：建栅栏 / 移条目 / 快照（app::semantics + item 表）
```

- IPC 新增 `semantics.plan`（只读预览）与 `semantics.apply`（快照后落地）
- 设置页「整理规则 → 智能整理」：预览 → 确认并整理
- 规则编辑器补上「公司名包含」，此前只能靠命令行
- 新条目自动归类：语义先判，判不出来的才交给属性规则

### 刻意保留的边界

1. **语义优先于属性规则（针对新条目）。** 语义能判的先进它认出的组，判不出来的才落到规则。
   反过来不行：默认规则「程序与快捷方式」「文件与文档」是全捕条件，规则先判会让语义
   一条都碰不到——第一版就是这样，桌面上仍然只有「程序」「文件与文档」两个大口袋。
2. **单条目只进已有栅栏。** 新建栅栏需要至少两个条目，避免栅栏碎片化。
3. **落地必须先存快照。** 与 `items.move` 同一条安全约束，失败即中止并给回滚命令。
4. **整理后清掉空栅栏。** 语义搬空的栅栏删除，指向它的规则改为放入「桌面」并保留，
   用户可以改回去；快照回不了规则，所以不删规则。

### 与 §十三 的偏差

- 「PecoFence 真机集成未验证」已消除：`semantics.plan`/`apply` 在真机上跑通
  （38 条桌面，7 组，36 个条目，184 ms，新建 7 个栅栏），新条目自动归类也实测通过。
- 仍然未验证：真实模型（`bench` 不传 `--assume-accuracy`）、多显示器与非 100% 缩放的栅栏布局。

---

## 附：历史方案

v0.3 的完整桌面工具方案见 git 提交 `de2a186`。保留原因：其中的采集层设计、分类优先级链、隐私规则、评测方案仍然有效，只是执行层整体替换。
