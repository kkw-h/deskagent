# 桌面图标整理工具 · 产品功能规划

> 版本 v0.2（决策落定版）｜ 2026-10-28 ｜ 平台：Windows 10/11，仅主屏

---

## 一、产品定义

**让 Agent 看懂你的桌面，然后把它重新摆好。**

不做"一键清理"式的粗暴归档，而是把桌面重构成一个**有语义分区的空间**：游戏一块、开发一块、影音一块，每个区域是一个可拖动的收纳盒，Agent 负责判断每个图标属于哪。

目标用户：桌面 30 个图标以上，同时装着游戏 / 办公 / 开发 / 影音多类软件，还堆着下载文件和截图的人。

---

## 二、已定决策（v0.2）

| # | 决策项 | 结论 |
|---|---|---|
| 1 | 首版范围 | **布局 + 移动都做**（分阶段交付，布局先落地） |
| 2 | LLM 接入 | **云端 + 本地都要**，可切换 |
| 3 | 分组命名 | **自由命名**（配收敛机制，见 6.2） |
| 4 | 桌面收纳盒 | **做**（视觉容器 + 物理收纳两档） |
| 5 | 技术栈 | **Rust** |
| 6 | 显示器 | **仅主屏** |

---

## 三、核心设计原则

### 3.1 两层动作，风险分级

| | 图标布局 | 文件移动 |
|---|---|---|
| 改动对象 | Shell 图标坐标 | 文件系统 |
| 风险 | 零 | 中 |
| 回滚 | 秒级 | 依赖日志 |
| 默认行为 | 直接执行 | 需逐条确认 |

两者都做，但**始终分开呈现**：预览界面上，布局变化是"图标挪了位置"，移动变化是"文件进了文件夹"，用不同视觉标识。用户永远知道自己即将失去什么控制权。

### 3.2 LLM 只做判断，不做动作

Agent 输出分类方案 JSON，所有动作由确定性代码执行：校验 → 快照 → 执行 → 记录。

```
Agent：「这几个是游戏」→ {group: "游戏", items: [id1, id2]}
程序：「收到」→ 校验存在性 → 生成快照 → 执行 → 返回 snapshot_id
```

幻觉最多导致分类不准（预览可见），不会导致文件丢失。

### 3.3 收纳盒是视觉优先的

收纳盒默认是**视觉容器**：桌面图标被框在区域内，文件一个都不动。用户可以在设置里把某个盒子升级为**物理收纳**（文件真的移进 `Desktop/游戏/`）。

这个默认值决定了产品第一印象：打开就能用，不怕。

### 3.4 一切可逆

每个操作生成快照（图标坐标 + 文件位置 + 分组定义）。撤销是产品的一级功能，不是隐藏菜单。

---

## 四、用户场景

**A · 首次整理**
桌面 47 个图标 → 3 秒扫描 → 浮层显示 5 个分区色块和归属预览 → 拖拽微调 → 确认 → 图标飞向各自区域。全程无文件移动。

**B · 自然语言微调**
「把微信和飞书放一起」「游戏都挪到右下角」「这几个截图收进一个盒子」→ Agent 解析 → 预览 → 执行。

**C · 增量守护**
新装软件，桌面多一个图标 → 托盘提示「发现 1 个新图标，归到『开发工具』？」→ 点一下归位。

**D · 物理归档**
用户说「把桌面上的文档都收进文档盒」→ 提示"这将移动 12 个文件到 Desktop/文档/"→ 确认 → 执行 → 可撤销。

**E · 外部 Agent 调用**
在 WorkBuddy / Claude 里说「整理我的桌面」→ 调用 MCP 工具 → 预览 → 确认 → 执行。

---

## 五、功能地图

```
L1 采集    桌面枚举 · 快捷方式深解析 · PE 元数据 · 图标提取 · 坐标快照
L2 理解    信号加权 · 规则引擎(离线) · 语义引擎(LLM 双模式) · 修正学习 · 置信度分级
L3 布局    分区规划 · 收纳盒 · 全屏预览 · 拖拽微调
L4 执行    图标定位 · 文件移动 · 物理收纳 · 快照 · 回滚
L5 接入    内置 Agent · MCP Server · 托盘常驻 · 增量检测
```

---

## 六、模块详述

### 6.1 采集层

**桌面枚举**
- 路径从注册表 `User Shell Folders\Desktop` 读，**不能硬编码**（OneDrive 重定向后会变）
- 同时扫 `%PUBLIC%\Desktop`
- 过滤 `desktop.ini`、隐藏项、系统项
- 额外枚举 Shell 虚拟图标（此电脑、回收站、网络）

**快捷方式深解析**（核心技术点）

`.lnk` 要挖出五层信息：

| 层级 | 字段 | 来源 |
|---|---|---|
| 1 | 目标路径 / 参数 / 工作目录 | IShellLink::GetPath / GetArguments / GetWorkingDirectory |
| 2 | 公司名 / 产品名 / 文件描述 / 版本 | PE VersionInfo 资源 |
| 3 | 数字签名主体 | WinVerifyTrust + CryptQueryObject |
| 4 | 图标图像 | IShellItemImageFactory |
| 5 | 创建 / 修改时间 | 文件系统 |

`.url` 走 INI 解析，`.appref-ms`（ClickOnce）单独处理。

**图标提取**
`IShellItemImageFactory::GetImage` 取 256×256，转 PNG 缓存，按文件哈希失效。

**坐标快照**
COM 链路读每个图标当前 (x, y)，作为回滚基准。

> 链路：`CLSID_ShellWindows` → `IShellWindows::FindWindowSW(SWC_DESKTOP)` → `IServiceProvider::QueryService(SID_STopLevelBrowser)` → `IShellBrowser::QueryActiveShellView` → `IFolderView2`

### 6.2 理解层

**信号加权融合**

```
快捷方式目标路径       0.30   ← 最强
PE 版本信息(公司/产品)  0.25
文件名关键词           0.15
数字签名主体           0.10
图标视觉特征           0.10
时间 / 大小            0.05
用户历史修正           0.05
```

**文件名是最弱信号。** 你桌面上 `Sciland`、`Yaak`、`Zed`、`夸克` 四个，光看名字判断不了——必须靠目标路径和 PE 信息。

**规则引擎**（离线，零延迟）
用户可编辑 YAML：
```yaml
- match: { target_contains: "\\Steam\\" }
  group: 游戏
  priority: 100
- match: { company: "Valve Corporation" }
  group: 游戏
- match: { ext: [".psd", ".ai", ".sketch"] }
  group: 设计
```

**语义引擎（双模式）**
- 云端：OpenAI 兼容协议，可配 base_url
- 本地：Ollama，默认 `qwen2.5:7b` 级别即可（分类任务不需要大模型）
- 切换粒度：全局默认 + 单次覆盖
- 隐私：只上传元数据（名称 / 目标路径 / 公司名），**不上传文件内容**

**自由命名的收敛机制**（对应决策 #3）

自由命名最大的坑是每次跑出不同的分类体系：这次是「游戏」，下次是「游戏娱乐」，再下次是「Games」，桌面越整越碎。所以配三层收敛：

1. **上下文注入**：每次调用 LLM 时，把当前已有的分组列表作为上下文传入，要求"优先复用已有分组，确有必要才新建"
2. **相似度合并**：新分组名与已有分组做编辑距离 + 语义相似度比对，超过阈值时提示"是否并入『游戏』？"
3. **分组字典沉淀**：所有用过的分组名持久化到本地 DB，成为用户的长期分类资产。用户可手动重命名 / 合并 / 删除

**置信度分级**
- ≥0.85 自动归入
- 0.5–0.85 归入但标记待确认
- <0.5 进「待归类」区，不猜测

**修正学习**
记录用户每次手动拖动的 (item, from, to)。同类修正出现 3 次，自动升格为永久规则。

### 6.3 布局层

**分区规划**
桌面划为若干矩形区域。只处理主屏，坐标体系简化为单一屏幕矩形（仍需处理 DPI 缩放，进程声明 Per-Monitor V2 DPI aware，坐标统一用物理像素）。

**收纳盒**（对应决策 #4）

两档形态，默认轻档：

| 形态 | 实现 | 文件是否移动 |
|---|---|---|
| 轻收纳盒（默认） | 桌面覆盖层绘制半透明圆角容器，图标坐标被约束在容器内 | 否 |
| 重收纳盒（可选） | 在 `Desktop/` 下建真实文件夹，文件移入 | 是 |

轻收纳盒的视觉要素：
- 半透明圆角底板 + 顶部标题栏（名称 + 图标数）
- 可拖动、可缩放、可折叠
- 折叠后只显示标题栏，图标收进盒子（坐标堆叠在标题栏下方）
- 双击标题栏重命名
- 右键菜单：转为物理收纳 / 解散 / 改色

**全屏预览浮层**
半透明覆盖真实桌面，显示分区色块和图标归属。可拖分区、拖单个图标跨区、双击改名、调整大小。

**这是产品体验的分水岭。** 用户看到"整理后长什么样"才敢点确认。

### 6.4 执行层

**图标定位**（零风险）
`IFolderView2::SelectAndPositionItems` 批量设置坐标，不动文件系统。
- 执行前存完整坐标快照
- 处理 DPI 缩放
- 监听 `WM_SETTINGCHANGE`，Explorer 重启或分辨率变化后重新应用

**文件移动**（需确认，对应决策 #1）
- 移动前检查：文件是否被占用、路径长度、是否有进程引用
- 快捷方式移动后验证目标仍可达
- 逐条列出待移动文件，用户确认后执行
- 全程记录 src / dst / 时间 / snapshot_id

**快照与回滚**
每次执行生成快照（图标坐标 + 文件位置 + 分组定义）存 SQLite。支持一键回滚上次、历史快照列表、自动清理 30 天前快照。

### 6.5 接入层

**内置 Agent**：App 内对话入口
**MCP Server**：能力暴露给外部 Agent
**托盘常驻**：开机自启、新图标检测、快速整理

---

## 七、Agent 工具契约（MCP）

| 工具 | 作用 | 风险级 |
|---|---|---|
| `desktop_scan` | 扫描桌面，返回结构化清单 | 只读 |
| `desktop_get_icon` | 取条目图标 base64 | 只读 |
| `desktop_list_groups` | 列出已有分组（供 LLM 复用） | 只读 |
| `desktop_propose_layout` | 生成分组方案 | 只读 |
| `desktop_preview_layout` | 渲染预览图 | 只读 |
| `desktop_apply_layout` | 执行图标布局（需 plan_id） | 写·可回滚 |
| `desktop_move_items` | 移动文件（需 plan_id + confirm） | 写·需确认 |
| `desktop_undo` | 回滚到快照 | 写·可回滚 |
| `desktop_list_snapshots` | 历史快照 | 只读 |
| `desktop_set_rule` | 增改分类规则 | 写 |

**硬约束**
1. 写操作必须先有 `plan_id`，plan 由 propose 生成且带 10 分钟过期
2. `move_items` 额外要求 `confirm: true`，且返回逐条结果
3. **不提供删除文件能力**——删除只能用户手动做
4. 错误返回机器可读 code

---

## 八、数据模型

```
Item      id, name, path, kind, target_path, company, product,
          ext, size, mtime, icon_hash, current_x, current_y

Group     id, name, color, rect(x,y,w,h), order, mode(light|physical),
          physical_path, is_system

Plan      id, created_at, expires_at, strategy, groups[], assignments[],
          status(draft|previewed|applied|expired)

Snapshot  id, plan_id, created_at, icon_positions[], file_moves[],
          group_defs[], note

Rule      id, match(json), group, priority, source(user|learned|builtin)

History   id, action, item_id, from, to, at, snapshot_id
```

存储：SQLite，`%LOCALAPPDATA%\DeskAgent\data.db`

---

## 九、安全模型

**操作分级**

| 级 | 内容 | 确认 |
|---|---|---|
| L0 | 扫描 / 预览 / 提案 | 无需 |
| L1 | 图标定位 | 预览后一次确认，秒回滚 |
| L2 | 文件移动 / 物理收纳 | 逐条列出 + 明确确认 |
| L3 | 删除 | **不提供** |

**硬性保护**
- `.lnk` / `.url` 永不删除，只移动
- 桌面一级文件夹不参与自动移动
- 敏感文件（文件名含"密码""身份证""简历""合同""invoice""passport"等）默认排除
- 被进程占用的文件跳过并提示
- 所有移动记录源路径，保证可逆

**隐私**
- 扫描结果只在本机处理
- 走云端 LLM 时只传元数据，不传文件内容
- 支持纯离线模式（规则引擎 + 本地模型）

---

## 十、技术方案：Rust

### 10.1 结论

**可行，推荐。** 但有几个成本点必须提前认账。

### 10.2 项目结构

```
deskagent/
├── Cargo.toml              # workspace
├── crates/
│   ├── shell-win/          # Windows Shell COM 封装（唯一 unsafe 集中区）
│   ├── core/               # 采集 / 分类 / 布局 / 执行，无 UI 依赖
│   ├── storage/            # SQLite 持久化
│   ├── mcp/                # MCP Server（stdio + http）
│   └── app/                # Tauri 2 桌面应用
└── rules/                  # 内置分类规则 YAML
```

关键：`core` 不依赖 Tauri，这样 `app` 和 `mcp` 都能复用它。

### 10.3 依赖选型

```toml
[workspace.dependencies]
windows = { version = "0.58", features = [
  "Win32_Foundation", "Win32_System_Com", "Win32_System_Ole",
  "Win32_UI_Shell", "Win32_UI_Shell_Common", "Win32_UI_WindowsAndMessaging",
  "Win32_Storage_FileSystem", "Win32_Security_WinTrust",
  "Win32_Graphics_Gdi", "Win32_System_Registry",
] }
tauri = { version = "2", features = ["tray-icon"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
image = "0.25"
notify = "6"
tokio = { version = "1", features = ["full"] }
rmcp = "0.2"
thiserror = "2"
anyhow = "1"
tracing = "0.1"
```

### 10.4 关键技术路径

**Shell COM 调用链（Rust）**
```
CoCreateInstance(CLSID_ShellWindows)
 → IShellWindows::FindWindowSW(CSIDL_DESKTOP, SWC_DESKTOP, &hwnd, SWFO_NEEDDISPATCH)
 → IDispatch::QueryInterface(IServiceProvider)
 → QueryService(SID_STopLevelBrowser, IShellBrowser)
 → IShellBrowser::QueryActiveShellView → IShellView
 → QueryInterface(IFolderView2)
 → Item(i) / GetItemPosition(pidl) / SelectAndPositionItems(...)
```

**快捷方式解析**
`CoCreateInstance(CLSID_ShellLink)` → `IPersistFile::Load(path)` → `IShellLinkW::GetPath/GetArguments/GetWorkingDirectory`

**PE 版本信息**
`GetFileVersionInfoSizeW` → `GetFileVersionInfoW` → `VerQueryValueW` 查 `\StringFileInfo\<langcp>\CompanyName` 等字段

**签名验证**
`WinVerifyTrust` + `CryptQueryObject` + `CertGetNameString`

**图标提取**
`SHCreateItemFromParsingName` → `IShellItemImageFactory::GetImage(SIZE{256,256}, SIIGBF_BIGGERSIZEOK)` → HBITMAP → `image` crate 转 PNG

### 10.5 需要认账的成本

| 成本点 | 说明 | 缓解 |
|---|---|---|
| COM 样板代码多 | `windows` crate 的 COM 比 C# 繁琐，PIDL 生命周期要手动 `CoTaskMemFree` | 全部封在 `shell-win` 一个 crate，对外暴露安全 Rust API |
| unsafe 密集 | COM 调用、GDI 句柄、注册表都要 unsafe | 同上，`#![deny(unsafe_op_in_unsafe_fn)]`，单点收口 |
| 全屏透明浮层 | Tauri 透明窗口 + 点击穿透需要 `WS_EX_LAYERED` + `WS_EX_TRANSPARENT` | 用 `windows` crate 直接改扩展窗口样式 |
| 构建环境 | 需要 rustup + MSVC Build Tools（约 3GB） | 一次性安装 |
| 图标坐标语义 | 逻辑坐标 vs 物理像素，DPI 缩放易错 | 进程声明 Per-Monitor V2，统一物理像素 |

**替代方案**：如果 COM 部分卡住，可以只把 `shell-win` 换成 C# 写的 sidecar 进程（JSON-RPC 通信），其余保持 Rust。但建议先试纯 Rust，`windows` crate 的 Shell 覆盖是完整的。

### 10.6 备选 GUI 框架

| 方案 | 优点 | 缺点 |
|---|---|---|
| **Tauri 2**（推荐） | UI 用 Web 技术，浮层预览好做，生态成熟 | 包体略大 |
| egui | 纯 Rust，单文件小 | 做精细浮层 UI 吃力 |
| iced | 纯 Rust，架构清晰 | 生态不如 Tauri |

---

## 十一、路线图

范围比 v0.1 大（多了移动 + 收纳盒 + 双 LLM），但**交付顺序不变：布局先落地**。这是风险控制，不是功能裁剪。

**M0 · 可行性验证（1–2 周）**
- 装 rustup + MSVC Build Tools，搭 workspace
- 跑通 `IFolderView2` 读坐标 + 写坐标
- 跑通 `.lnk` 解析（目标路径 + PE 版本信息）
- 验证 DPI 缩放下的坐标正确性
- **产出：能把图标按规则摆成三列的控制台 Demo**

**M1 · MVP（3 周）**
- 桌面扫描 + 深解析 + 图标提取
- 规则引擎（内置 30 条常见软件规则）
- 分区布局 + 全屏预览浮层 + 拖拽微调
- 轻收纳盒（视觉容器）
- 执行 + 快照 + 一键撤销
- 托盘常驻
- **范围：只做 L1 布局，不动文件**

**M2 · 移动与收纳（3 周）**
- 文件移动能力 + 逐条确认 UI
- 重收纳盒（物理收纳）
- 完整快照体系 + 历史恢复
- 占用检测、敏感文件排除

**M3 · 智能（3–4 周）**
- LLM 双模式接入（云端 + Ollama）
- 自由命名 + 三层收敛机制
- 自然语言指令
- 修正学习

**M4 · 生态（3 周）**
- MCP Server 发布
- 规则市场
- 工作 / 娱乐模式切换
- 增量守护

---

## 十二、风险清单

| 风险 | 影响 | 应对 |
|---|---|---|
| Explorer 重启后图标被系统重排 | 布局失效 | 监听 `WM_SETTINGCHANGE` 自动重应用；提供"锁定布局" |
| 分辨率 / 缩放变化导致坐标错乱 | 图标堆叠 | 快照里记录当时的屏幕参数，变化时提示重排 |
| 自由命名导致分类碎片化 | 越整越乱 | 三层收敛机制（6.2） |
| LLM 分类不稳定 | 每次结果不同 | 低温度 + 分组上下文注入 + 规则优先 |
| 移动文件破坏用户引用 | 用户投诉 | 逐条确认 + 完整回滚 + 敏感文件排除 |
| Rust COM 开发成本超预期 | 延期 | `shell-win` 单点收口；必要时降级为 C# sidecar |
| 桌面覆盖层与 Explorer 抢焦点 | 交互异常 | 浮层用 `WS_EX_NOACTIVATE`，仅在预览态显示 |

---

## 附：桌面实测样本（分类引擎验收基准）

当前 35 个条目，全部为快捷方式 / 链接：

```
游戏      Steam / WeGame / Epic Games Launcher / 英雄联盟WeGame版 /
          双点博物馆.url / 双点校园.url / 游戏加加轻量版 / 雷神加速器 / UU远程
开发      Zed / Yaak / VMware Workstation Pro / 微信开发者工具 / WorkBuddy AI
办公      WPS Office / 飞书 / 微信 / QQ / Telegram / 小米互联服务
影音      PotPlayer 64 bit / 网易云音乐 / 哔哩哔哩 / 夸克
工具      Clash Verge / ToDesk / QuickLook / OBS Studio / Sciland
浏览器    Google Chrome / Microsoft Edge / kw - Chrome / 豆包
```

`Sciland`、`Yaak`、`Zed`、`夸克` 四个光看名字无法分类，是引擎验收的关键样本。
