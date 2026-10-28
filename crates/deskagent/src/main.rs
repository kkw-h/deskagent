use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use deskagent_classify::{Item, RulePack};
use deskagent_compile as compile;
use deskagent_llm as llm;
use deskagent_pecofence as pecofence;

/// 内置规则包随二进制发布，避免运行期依赖文件路径。
const BUILTIN_RULES: &str = include_str!("../../../rules/builtin.yaml");

#[derive(Parser)]
#[command(name = "deskagent", version, about = "给桌面整理器装上语义大脑")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 扫描目录，输出条目清单
    Scan {
        /// 扫描指定目录，可重复；缺省时扫桌面
        #[arg(long, value_name = "DIR")]
        dir: Vec<PathBuf>,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 扫描并分类，输出分组结果
    Classify {
        /// 扫描指定目录，可重复；缺省时扫桌面
        #[arg(long, value_name = "DIR")]
        dir: Vec<PathBuf>,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 用带标注的样本评测规则包
    Eval {
        /// 扫描指定目录，可重复；缺省时扫桌面
        #[arg(long, value_name = "DIR")]
        dir: Vec<PathBuf>,
        /// 改用 JSON 条目文件作为样本源（与 --dir 二选一）
        #[arg(long, value_name = "FILE")]
        input: Option<PathBuf>,
        /// 标注文件（YAML），键为文件名或条目名，值为人工认定的分组
        #[arg(long, value_name = "FILE")]
        labels: PathBuf,
        /// 使用外部规则包，缺省用内置
        #[arg(long, value_name = "FILE")]
        rules: Option<PathBuf>,
        /// 只评测某个切分：dev / test / all
        #[arg(long, default_value = "all")]
        split: String,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 分类外部 JSON 条目（不扫盘），用于评测非快捷方式来源
    ClassifyJson {
        /// 输入文件，形如 [{"name":"...","target":"...","company":"...","ext":"..."}]
        #[arg(long, value_name = "FILE")]
        input: PathBuf,
        /// 使用外部规则包，缺省用内置
        #[arg(long, value_name = "FILE")]
        rules: Option<PathBuf>,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 生成整理方案，不触碰系统
    Plan {
        /// 扫描指定目录，可重复；缺省时扫桌面
        #[arg(long, value_name = "DIR")]
        dir: Vec<PathBuf>,
        /// 少于这个条目数的分组不建栅栏
        #[arg(long, default_value_t = 2)]
        min_items: usize,
        /// 规则判不出来的条目交给模型补判断
        #[arg(long)]
        llm: bool,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 推导可以沉淀成 PecoFence 持久规则的扩展名规则
    Rules {
        /// 扫描指定目录，可重复；缺省时扫桌面
        #[arg(long, value_name = "DIR")]
        dir: Vec<PathBuf>,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 增量守护：桌面出现新图标就分类并归位
    Watch {
        /// 处理一个事件就退出，便于脚本循环调用
        #[arg(long)]
        once: bool,
    },
    /// 用模型给规则包没覆盖到的条目补判断
    Resolve {
        /// 扫描指定目录，可重复；缺省时扫桌面
        #[arg(long, value_name = "DIR")]
        dir: Vec<PathBuf>,
        /// 连规则已覆盖的条目也一并问模型，用于对比
        #[arg(long)]
        all: bool,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 输出机器可读的命令目录，供外部 agent 自描述发现
    Describe,
    /// 输出给编码 agent 用的使用指南
    Skill,
    /// 对照实验：手写规则与模型分类各需要多少次人工操作
    Bench {
        /// 扫描指定目录，可重复；缺省时扫桌面
        #[arg(long, value_name = "DIR")]
        dir: Vec<PathBuf>,
        /// 改用 JSON 条目文件作为样本源
        #[arg(long, value_name = "FILE")]
        input: Option<PathBuf>,
        /// 标注文件（YAML）
        #[arg(long, value_name = "FILE")]
        labels: PathBuf,
        /// 不调用模型，直接假设一个准确率来估算
        #[arg(long, value_name = "P")]
        assume_accuracy: Option<f64>,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 把方案落地到 PecoFence
    Apply {
        /// 扫描指定目录，可重复；缺省时扫桌面
        #[arg(long, value_name = "DIR")]
        dir: Vec<PathBuf>,
        /// 少于这个条目数的分组不建栅栏
        #[arg(long, default_value_t = 2)]
        min_items: usize,
        /// 覆盖自动探测的工作区，格式 x,y,w,h（物理像素，虚拟屏坐标）
        #[arg(long, value_name = "RECT")]
        area: Option<String>,
        /// 同时把扩展名类判断沉淀成 PecoFence 持久规则
        #[arg(long)]
        with_rules: bool,
        /// 规则判不出来的条目交给模型补判断
        #[arg(long)]
        llm: bool,
        /// 只演示，不修改任何东西
        #[arg(long)]
        dry_run: bool,
        /// 确认执行。没有这个标志时只做 dry-run
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Scan { dir, json } => scan(&dir, json),
        Cmd::Classify { dir, json } => classify(&dir, json),
        Cmd::Eval {
            dir,
            input,
            labels,
            rules,
            split,
            json,
        } => {
            let items = match &input {
                Some(p) => load_json_items(p)?,
                None => collect(&dir)?,
            };
            eval(&items, &labels, rules.as_deref(), &split, json)
        }
        Cmd::ClassifyJson {
            input,
            rules,
            json,
        } => classify_json(&input, rules.as_deref(), json),
        Cmd::Plan {
            dir,
            min_items,
            llm,
            json,
        } => plan(&dir, min_items, llm, json),
        Cmd::Apply {
            dir,
            min_items,
            area,
            with_rules,
            llm,
            dry_run,
            yes,
        } => apply(&dir, min_items, area.as_deref(), with_rules, llm, dry_run, yes),
        Cmd::Rules { dir, json } => rules(&dir, json),
        Cmd::Watch { once } => watch(once),
        Cmd::Resolve { dir, all, json } => resolve(&dir, all, json),
        Cmd::Describe => describe(),
        Cmd::Skill => skill(),
        Cmd::Bench {
            dir,
            input,
            labels,
            assume_accuracy,
            json,
        } => bench(&dir, input.as_deref(), &labels, assume_accuracy, json),
    }
}

// ---------------------------------------------------------------- 方案

fn build_plan(dirs: &[PathBuf], min_items: usize, use_llm: bool) -> anyhow::Result<compile::Plan> {
    let items = collect(dirs)?;
    let pack = RulePack::from_yaml(BUILTIN_RULES)?;
    let extra = if use_llm {
        llm_extras(&items, &pack)?
    } else {
        Vec::new()
    };
    let opts = compile::Options { min_items };
    Ok(compile::build_with(&items, &pack, &opts, &extra))
}

/// 候选分组。模型只能从这里选，避免它发明出 PecoFence 里不存在的栅栏。
fn candidate_groups() -> Vec<String> {
    compile::GROUP_ORDER.iter().map(|s| s.to_string()).collect()
}

/// 规则判不出来的条目交给模型，返回 `(file_name, group)`。
///
/// 规则优先：调用方只在规则返回 None 时才用这里的结果，
/// 确定性判断不会被模型覆盖。
fn llm_extras(items: &[Item], pack: &RulePack) -> anyhow::Result<Vec<(String, String)>> {
    let cfg = llm::Config::from_env().ok_or_else(|| {
        anyhow::anyhow!(
            "没有配置模型。设置 DESKAGENT_LLM_BASE_URL（本地 Ollama 用 http://127.0.0.1:11434/v1）"
        )
    })?;
    let pending: Vec<Item> = items
        .iter()
        .filter(|i| pack.classify(i).is_none())
        .cloned()
        .collect();
    if pending.is_empty() {
        return Ok(Vec::new());
    }

    let client = llm::Client::new(cfg.clone());
    eprintln!("模型（{}）：待判断 {} 条", cfg.describe_mode(), pending.len());
    let suggestions = client.classify(&pending, &candidate_groups())?;
    Ok(suggestions
        .into_iter()
        .filter_map(|s| s.group.map(|g| (s.file_name, g)))
        .collect())
}

fn plan(dirs: &[PathBuf], min_items: usize, use_llm: bool, json: bool) -> anyhow::Result<()> {
    let p = build_plan(dirs, min_items, use_llm)?;

    if json {
        let payload = serde_json::json!({
            "total": p.total,
            "moved": p.moved(),
            "fences": p.fences.iter().map(|f| serde_json::json!({
                "title": f.title,
                "count": f.count(),
                "items": f.items,
            })).collect::<Vec<_>>(),
            "skipped": p.skipped.iter().map(|s| serde_json::json!({
                "name": s.name, "reason": s.reason,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_plan(&p);
    Ok(())
}

fn print_plan(p: &compile::Plan) {
    println!(
        "条目 {}  归入栅栏 {}  留在桌面 {}",
        p.total,
        p.moved(),
        p.skipped.len()
    );
    println!("{}", "-".repeat(76));
    for f in &p.fences {
        println!("{}（{}）", f.title, f.count());
        for path in &f.items {
            println!("    {}", file_label(path));
        }
    }
    if !p.skipped.is_empty() {
        println!("{}", "-".repeat(76));
        println!("留在桌面（{}）", p.skipped.len());
        for s in &p.skipped {
            println!("    {:<24} {}", s.name, s.reason);
        }
    }
}

fn file_label(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

// ---------------------------------------------------------------- 规则编译

fn rules(dirs: &[PathBuf], json: bool) -> anyhow::Result<()> {
    let items = collect(dirs)?;
    let pack = RulePack::from_yaml(BUILTIN_RULES)?;
    let c = compile::rules::compile_ext_rules(&items, &pack);

    if json {
        let payload = serde_json::json!({
            "rules": c.rules.iter().map(|r| serde_json::json!({
                "name": r.name(),
                "group": r.group,
                "exts": r.exts,
            })).collect::<Vec<_>>(),
            "conflicts": c.conflicts.iter().map(|x| serde_json::json!({
                "ext": x.ext,
                "groups": x.groups.iter().map(|(g, n)| serde_json::json!({
                    "group": g, "count": n,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "skipped_semantic": c.skipped_semantic,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_rules(&c);
    Ok(())
}

fn print_rules(c: &compile::rules::Compiled) {
    println!("可沉淀成持久规则的扩展名：{}", c.rules.len());
    println!("{}", "-".repeat(76));
    for r in &c.rules {
        println!(
            "{:<24} {} → {}",
            r.name(),
            r.exts.join(" "),
            r.group
        );
    }
    if !c.conflicts.is_empty() {
        println!("{}", "-".repeat(76));
        println!("不编译（同一扩展名落在多个分组，本身不携带分类信息）");
        for x in &c.conflicts {
            let detail = x
                .groups
                .iter()
                .map(|(g, n)| format!("{g}×{n}"))
                .collect::<Vec<_>>()
                .join("、");
            println!("    {:<8} {detail}", x.ext);
        }
    }
    if c.skipped_semantic > 0 {
        println!("{}", "-".repeat(76));
        println!(
            "{} 个条目（快捷方式等）靠语义判断，无法表达成 PecoFence 规则，只能一次性移动。",
            c.skipped_semantic
        );
        println!("这部分的新增图标要靠 `deskagent watch` 守护。");
    }
}

/// 安装规则。只能给**已存在**的栅栏装，否则 PecoFence 会报 `fence_not_found`。
fn install_rules(cli: &pecofence::Cli, dirs: &[PathBuf], allowed: &[String]) -> anyhow::Result<()> {
    let items = collect(dirs)?;
    let pack = RulePack::from_yaml(BUILTIN_RULES)?;
    let c = compile::rules::compile_ext_rules(&items, &pack);

    let usable: Vec<&compile::rules::ExtRule> = c
        .rules
        .iter()
        .filter(|r| allowed.contains(&r.group))
        .collect();
    if usable.is_empty() {
        println!("\n没有可安装的扩展名规则。");
        return Ok(());
    }

    let list = cli.list_rules()?;
    let existing = compile::rules::existing_rule_names(&list);
    let keep_updated = compile::rules::keep_updated(&list);

    println!("\n安装扩展名规则：");
    let mut added = 0usize;
    for r in usable {
        let name = r.name();
        if existing.contains(&name) {
            println!("    「{name}」已存在，跳过");
            continue;
        }
        match cli.add_ext_rule(&name, &r.group, &r.exts) {
            Ok(_) => {
                println!("    已添加 {name}：{} → {}", r.ext_arg(), r.group);
                added += 1;
            }
            Err(e) => eprintln!("    添加 {name} 失败：{e}"),
        }
    }

    if added > 0 && !keep_updated {
        // 这条提示很关键：规则装了但自动归档关着，用户会以为没生效
        println!(
            "    注意：PecoFence 的「新条目自动归档」（keepUpdated）当前是关闭的，\n\
             扩展名规则不会自动生效。请在 PecoFence 设置里打开，或让 `deskagent watch` 常驻。"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- 增量守护

fn watch(once: bool) -> anyhow::Result<()> {
    let cli = pecofence::Cli::discover()?;
    let pack = RulePack::from_yaml(BUILTIN_RULES)?;

    // 先确认应用在运行，否则 watch 会一直阻塞在连接上
    cli.status().map_err(|e| match e {
        pecofence::Error::NotRunning => anyhow::anyhow!("PecoFence 没有在运行，请先启动它"),
        other => anyhow::anyhow!(other),
    })?;

    println!("增量守护已启动。桌面出现新图标时自动分类归位，Ctrl+C 退出。");
    if once {
        println!("（--once：处理一个事件后退出）");
    }

    loop {
        let event = match cli.watch_inbox_once() {
            Ok(v) => v,
            Err(e) => {
                // 应用退出或超时都会到这里。once 模式直接失败，常驻模式退避重试
                if once {
                    return Err(e.into());
                }
                eprintln!("等待事件失败：{e}，2 秒后重试");
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
        };

        handle_event(&cli, &pack, &event);

        if once {
            return Ok(());
        }
    }
}

/// 处理一个 `item.added` 事件：重新采集元数据 → 分类 → 移到对应栅栏。
fn handle_event(cli: &pecofence::Cli, pack: &RulePack, event: &serde_json::Value) {
    let Some(path) = event
        .get("item")
        .and_then(|i| i.get("path"))
        .and_then(|p| p.as_str())
    else {
        // shell 命名空间条目（此电脑、回收站）没有路径，跳过
        return;
    };

    let p = Path::new(path);
    if !p.is_file() {
        return;
    }
    let Some(item) = to_item(p) else {
        return;
    };

    let Some((group, reason)) = pack.classify(&item) else {
        println!("{}：规则包未覆盖，留在桌面", item.name);
        return;
    };

    let fences = match cli.list_fences() {
        Ok(v) => pecofence::reusable_titles(&v),
        Err(e) => {
            eprintln!("读取栅栏列表失败：{e}");
            return;
        }
    };
    if !fences.contains(&group) {
        println!("{} → {group}（还没有这个栅栏，留在桌面）", item.name);
        return;
    }

    match cli.move_items(&[item.path.clone()], &group, false) {
        Ok(_) => println!("{} → {group}（{reason}）", item.name),
        Err(e) => eprintln!("{} 归位失败：{e}", item.name),
    }
}

fn apply(
    dirs: &[PathBuf],
    min_items: usize,
    area_override: Option<&str>,
    with_rules: bool,
    use_llm: bool,
    dry_run: bool,
    yes: bool,
) -> anyhow::Result<()> {
    let p = build_plan(dirs, min_items, use_llm)?;
    print_plan(&p);

    if p.fences.is_empty() {
        println!("\n没有达到建栅栏阈值的分组，无事可做。");
        return Ok(());
    }

    // PecoFence 不在场时也要能出方案，所以这里是 Option 而不是错误
    let cli = pecofence::Cli::discover().ok();
    let area = match area_override {
        Some(s) => parse_area(s).ok_or_else(|| anyhow::anyhow!("--area 格式应为 x,y,w,h"))?,
        None => cli
            .as_ref()
            .and_then(work_area)
            .unwrap_or_else(compile::fallback_area),
    };
    let rects = compile::layout(p.fences.len(), area);
    println!(
        "\n布局区域 x={} y={} w={} h={}，共 {} 个栅栏",
        area.x,
        area.y,
        area.w,
        area.h,
        p.fences.len()
    );

    let mutate = yes && !dry_run;
    if !mutate {
        println!("\n[dry-run] 以下动作会被执行，加 --yes 才会真正修改：");
        for (i, f) in p.fences.iter().enumerate() {
            println!(
                "    fence create --title {} --rect {}",
                f.title,
                rects[i].to_arg()
            );
            println!("    item move <{} 项> --to {}", f.count(), f.title);
        }
        if with_rules {
            let items = collect(dirs)?;
            let pack = RulePack::from_yaml(BUILTIN_RULES)?;
            let c = compile::rules::compile_ext_rules(&items, &pack);
            if c.rules.is_empty() {
                println!("    （没有可编译的扩展名规则）");
            }
            for r in &c.rules {
                println!(
                    "    rule add --name {} --ext {} --to {}",
                    r.name(),
                    r.ext_arg(),
                    r.group
                );
            }
        }
        println!("\n注意：item move 会改变图标所属栅栏，确认无误再加 --yes。");
        return Ok(());
    }

    // discover() 内部已经用 status 探过一次，这里不必再探
    let cli = cli.ok_or(pecofence::Error::NotRunning)?;
    println!("\n已连接 PecoFence");

    // 有副作用前先存快照，出问题能回滚布局
    let stamp = timestamp();
    let snap = cli.snapshot_save(&format!("deskagent-{stamp}"))?;
    let snap_id = snap
        .get("snapshot")
        .and_then(|s| s.get("id"))
        .and_then(|i| i.as_str())
        .unwrap_or("(未返回)")
        .to_string();
    println!("\n已保存布局快照 {snap_id}，回滚用：pecofence-cli snapshot restore {snap_id}");

    // 已存在的同名栅栏直接复用，避免重复运行时堆出一串同名栅栏。
    // 只复用普通栅栏：portal 背后是真实文件夹，往里 move 会移动真实文件。
    let list = cli.list_fences()?;
    let reusable = pecofence::reusable_titles(&list);
    let blocked: Vec<String> = pecofence::all_titles(&list)
        .into_iter()
        .filter(|t| !reusable.contains(t))
        .collect();

    let mut blocked_groups: Vec<String> = Vec::new();
    for (i, f) in p.fences.iter().enumerate() {
        if blocked.contains(&f.title) {
            // 再建一个同名栅栏会让 --to 变成 ambiguous_fence，只能整组跳过
            println!(
                "跳过「{}」：已有同名栅栏，但它是文件夹栅栏或标签页。往里搬会移动真实文件，或导致选择歧义。",
                f.title
            );
            blocked_groups.push(f.title.clone());
            continue;
        }
        if reusable.contains(&f.title) {
            println!("栅栏「{}」已存在，复用", f.title);
            continue;
        }
        if let Err(e) = cli.create_fence_rect(&f.title, &rects[i].to_arg()) {
            eprintln!("创建栅栏「{}」失败：{e}", f.title);
            return Err(e.into());
        }
        println!("已创建栅栏「{}」", f.title);
    }

    let mut failures = 0usize;
    for f in &p.fences {
        if blocked_groups.contains(&f.title) {
            continue;
        }
        match cli.move_items(&f.items, &f.title, false) {
            Ok(v) => {
                let changed = v.get("changed").and_then(|c| c.as_bool()).unwrap_or(false);
                if changed {
                    println!("已移入「{}」{} 项", f.title, f.count());
                } else {
                    println!("「{}」状态已是目标状态，未变动", f.title);
                }
                if let Some(w) = v.get("warning").and_then(|w| w.as_str()) {
                    println!("  提示：{w}");
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("移入「{}」失败：{e}", f.title);
            }
        }
    }

    println!();
    let done = p.fences.len() - blocked_groups.len();
    // 被跳过的分组不能算进已归位
    let moved: usize = p
        .fences
        .iter()
        .filter(|f| !blocked_groups.contains(&f.title))
        .map(|f| f.count())
        .sum();
    if failures == 0 {
        println!("完成。{done} 个栅栏，{moved} 项已归位。");
        if !blocked_groups.is_empty() {
            println!(
                "{} 个分组因标题冲突被跳过：{}",
                blocked_groups.len(),
                blocked_groups.join("、")
            );
        }
        println!("不满意可以回滚布局：pecofence-cli snapshot restore {snap_id}");

        if with_rules {
            let allowed: Vec<String> = p
                .fences
                .iter()
                .map(|f| f.title.clone())
                .filter(|t| !blocked_groups.contains(t))
                .collect();
            install_rules(&cli, dirs, &allowed)?;
        }
        Ok(())
    } else {
        anyhow::bail!("{failures} 个栅栏搬运失败，布局快照 {snap_id} 可用于回滚")
    }
}

fn parse_area(s: &str) -> Option<compile::Rect> {
    let parts: Vec<i64> = s
        .split(',')
        .map(|p| p.trim().parse::<i64>())
        .collect::<Result<_, _>>()
        .ok()?;
    if parts.len() != 4 {
        return None;
    }
    Some(compile::Rect {
        x: parts[0],
        y: parts[1],
        w: parts[2],
        h: parts[3],
    })
}

/// 从 `monitor list` 里取主屏工作区。
///
/// 文档只说明「shows each monitor's rect, workArea and dpi」，没有给出精确字段形状，
/// 所以这里同时接受 `{x,y,w,h}` 对象和 `[x,y,w,h]` 数组两种写法；
/// 都读不出来就返回 None，由调用方回退到保守的默认区域。
fn work_area(cli: &pecofence::Cli) -> Option<compile::Rect> {
    let v = cli.list_monitors().ok()?;
    let arr = v.as_array()?;
    let pick = arr
        .iter()
        .find(|m| m.get("primary").and_then(|p| p.as_bool()).unwrap_or(false))
        .or_else(|| arr.first())?;
    let wa = pick.get("workArea").or_else(|| pick.get("rect"))?;
    rect_of(wa)
}

fn rect_of(v: &serde_json::Value) -> Option<compile::Rect> {
    if let Some(a) = v.as_array() {
        if a.len() >= 4 {
            return Some(compile::Rect {
                x: a[0].as_i64()?,
                y: a[1].as_i64()?,
                w: a[2].as_i64()?,
                h: a[3].as_i64()?,
            });
        }
    }
    Some(compile::Rect {
        x: v.get("x")?.as_i64()?,
        y: v.get("y")?.as_i64()?,
        w: v.get("w")?.as_i64()?,
        h: v.get("h")?.as_i64()?,
    })
}

/// 快照名里的时间戳，避免同一秒重复运行时撞名。
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn scan(dirs: &[PathBuf], json: bool) -> anyhow::Result<()> {
    let items = collect(dirs)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    let with_target = items.iter().filter(|i| !i.target.is_empty()).count();
    let with_company = items.iter().filter(|i| !i.company.is_empty()).count();
    println!(
        "TOTAL={}  HAS_TARGET={} ({}%)  HAS_COMPANY={} ({}%)",
        items.len(),
        with_target,
        pct(with_target, items.len()),
        with_company,
        pct(with_company, items.len()),
    );
    println!("{}", "-".repeat(76));
    for i in &items {
        let mark = if i.target.is_empty() { "!" } else { " " };
        let company = if i.company.is_empty() { "-" } else { &i.company };
        println!("{mark} {:<22} {:<26} {}", i.name, trunc(company, 24), i.target);
    }
    Ok(())
}

fn classify(dirs: &[PathBuf], json: bool) -> anyhow::Result<()> {
    let items = collect(dirs)?;
    let pack = RulePack::from_yaml(BUILTIN_RULES)?;
    report(&pack, &items, json)
}

/// 外部 JSON 条目：字段可缺省，便于从注册表、清单文件等来源喂数据。
#[derive(serde::Deserialize)]
struct RawItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    company: String,
    #[serde(default)]
    ext: String,
}

fn classify_json(input: &Path, rules_path: Option<&Path>, json: bool) -> anyhow::Result<()> {
    let items = load_json_items(input)?;
    let rules_text = match rules_path {
        Some(p) => std::fs::read_to_string(p)?,
        None => BUILTIN_RULES.to_string(),
    };
    let pack = RulePack::from_yaml(&rules_text)?;
    report(&pack, &items, json)
}

/// 读取外部 JSON 条目。file_name 与 name 取同值，便于标注文件按名称索引。
fn load_json_items(input: &Path) -> anyhow::Result<Vec<Item>> {
    let raw = std::fs::read_to_string(input)?;
    let rows: Vec<RawItem> = serde_json::from_str(&raw)?;
    Ok(rows
        .into_iter()
        .map(|r| Item {
            file_name: r.name.clone(),
            name: r.name,
            path: String::new(),
            target: r.target,
            company: r.company,
            product: String::new(),
            ext: r.ext,
        })
        .collect())
}

fn report(pack: &RulePack, items: &[Item], json: bool) -> anyhow::Result<()> {
    let mut groups: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut unclassified = Vec::new();

    for item in items {
        match pack.classify(item) {
            Some((group, reason)) => {
                groups
                    .entry(group)
                    .or_default()
                    .push((item.name.clone(), reason));
            }
            None => unclassified.push(item.name.clone()),
        }
    }

    if json {
        let payload = serde_json::json!({
            "groups": groups
                .iter()
                .map(|(g, v)| serde_json::json!({
                    "group": g,
                    "count": v.len(),
                    "items": v.iter().map(|(n, _)| n).collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
            "unclassified": unclassified,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let classified = items.len() - unclassified.len();
    println!(
        "TOTAL={}  CLASSIFIED={} ({}%)  UNCLASSIFIED={}",
        items.len(),
        classified,
        pct(classified, items.len()),
        unclassified.len()
    );
    println!("{}", "-".repeat(76));
    for (group, members) in &groups {
        println!("{group}({})", members.len());
        for (name, reason) in members {
            println!("    {name:<22} {reason}");
        }
    }
    if !unclassified.is_empty() {
        println!("待归类({})", unclassified.len());
        for name in &unclassified {
            println!("    {name}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 模型补判

fn resolve(dirs: &[PathBuf], all: bool, json: bool) -> anyhow::Result<()> {
    let items = collect(dirs)?;
    let pack = RulePack::from_yaml(BUILTIN_RULES)?;
    let cfg = llm::Config::from_env().ok_or_else(|| {
        anyhow::anyhow!(
            "没有配置模型。\n\
             本地：DESKAGENT_LLM_BASE_URL=http://127.0.0.1:11434/v1 DESKAGENT_LLM_MODEL=qwen2.5\n\
             云端：再加 DESKAGENT_LLM_API_KEY"
        )
    })?;

    let pending: Vec<Item> = items
        .iter()
        .filter(|i| all || pack.classify(i).is_none())
        .cloned()
        .collect();

    println!("模型：{}", cfg.describe_mode());
    println!("待判断：{} 条（共 {} 条）", pending.len(), items.len());
    if pending.is_empty() {
        println!("没有需要模型判断的条目。");
        return Ok(());
    }
    if !cfg.is_local() {
        println!("注意：这是云端模式，条目信息（已去标识化）会离开本机。");
    }

    let client = llm::Client::new(cfg);
    let suggestions = client.classify(&pending, &candidate_groups())?;

    if json {
        let payload = serde_json::json!({
            "suggestions": suggestions.iter().map(|s| serde_json::json!({
                "file": s.file_name,
                "group": s.group,
                "reason": s.reason,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("{}", "-".repeat(76));
    let mut unsure = 0;
    for s in &suggestions {
        match &s.group {
            Some(g) => println!("{:<28} → {:<6} {}", s.file_name, g, s.reason),
            None => {
                unsure += 1;
                println!("{:<28} → 模型也拿不准    {}", s.file_name, s.reason);
            }
        }
    }
    println!("{}", "-".repeat(76));
    println!(
        "给出判断 {} 条，拿不准 {unsure} 条。拿不准的会留在桌面，不会瞎猜。",
        suggestions.len() - unsure
    );
    Ok(())
}

// ---------------------------------------------------------------- 自描述

fn describe() -> anyhow::Result<()> {
    let catalog = serde_json::json!({
        "name": "deskagent",
        "version": env!("CARGO_PKG_VERSION"),
        "purpose": "给桌面整理器装上语义大脑：读取桌面图标，判断每个是什么，编译成 PecoFence 能执行的动作",
        "requires": {
            "pecofence-cli": "M1 及之后的部分命令需要；缺省时 scan/classify/plan/rules/resolve 仍可用",
            "curl": "仅 --llm 相关命令需要，Windows 10 1803+ 自带",
        },
        "environment": {
            "PECOFENCE_CLI": "pecofence-cli 可执行文件路径，缺省从 PATH 查找",
            "DESKAGENT_LLM_BASE_URL": "OpenAI 兼容接口的基址，如 http://127.0.0.1:11434/v1",
            "DESKAGENT_LLM_MODEL": "模型名，缺省 gpt-4o-mini",
            "DESKAGENT_LLM_API_KEY": "云端模式需要；本地回环地址不需要",
            "DESKAGENT_LLM_TIMEOUT": "秒，缺省 60",
        },
        "commands": [
            {"name": "scan", "args": ["--dir <DIR>...", "--json"], "effect": "read-only",
             "desc": "扫描目录，输出条目与元数据覆盖"},
            {"name": "classify", "args": ["--dir <DIR>...", "--json"], "effect": "read-only",
             "desc": "扫描并分类"},
            {"name": "plan", "args": ["--dir <DIR>...", "--min-items <N>", "--llm", "--json"], "effect": "read-only",
             "desc": "生成整理方案，不触碰系统"},
            {"name": "rules", "args": ["--dir <DIR>...", "--json"], "effect": "read-only",
             "desc": "推导可沉淀成持久规则的扩展名规则"},
            {"name": "apply", "args": ["--dir <DIR>...", "--min-items <N>", "--area <RECT>", "--with-rules", "--llm", "--dry-run", "--yes"], "effect": "mutates",
             "desc": "建栅栏并搬运图标；无 --yes 时只 dry-run"},
            {"name": "watch", "args": ["--once"], "effect": "mutates",
             "desc": "常驻守护，新图标出现即分类归位"},
            {"name": "resolve", "args": ["--dir <DIR>...", "--all", "--json"], "effect": "network",
             "desc": "让模型给规则未覆盖的条目补判断"},
            {"name": "eval", "args": ["--dir <DIR>...", "--input <JSON>", "--labels <YAML>", "--rules <YAML>", "--split dev|test|all", "--json"], "effect": "read-only",
             "desc": "用带标注的样本评测规则包"},
            {"name": "bench", "args": ["--dir <DIR>...", "--input <JSON>", "--labels <YAML>", "--assume-accuracy <P>", "--json"], "effect": "read-only",
             "desc": "对照实验：手写规则与模型各需多少次人工操作"},
            {"name": "describe", "args": [], "effect": "read-only", "desc": "本目录"},
            {"name": "skill", "args": [], "effect": "read-only", "desc": "给编码 agent 的使用指南"},
        ],
        "safety": [
            "apply 在修改前一定先 snapshot save，输出里给出回滚命令",
            "apply 不复用 portal 栅栏：portal 背后是真实文件夹，item move 会移动真实文件",
            "模型只上传去标识化字段，路径仅保留产品段，不含用户名",
            "模型给出不存在的分组名会被丢弃，不会去创建不存在的栅栏",
        ],
    });
    println!("{}", serde_json::to_string_pretty(&catalog)?);
    Ok(())
}

fn skill() -> anyhow::Result<()> {
    println!("{SKILL_TEXT}");
    Ok(())
}

const SKILL_TEXT: &str = r#"# deskagent

给桌面整理器装上语义大脑。读取桌面图标，判断每个是什么，编译成 PecoFence 能执行的动作。

## 什么时候用

用户说「整理我的桌面」「把这些图标分一下类」「新装的软件又乱在桌面上了」时。

不要用它移动文件内容——它只改变图标属于哪个栅栏。

## 推荐的调用顺序

1. `deskagent plan` —— 先看方案。这一步不改任何东西，可以放心跑。
2. 把方案念给用户确认。分组名、每组的条目数、哪些留在桌面。
3. `deskagent apply` —— 仍然只是 dry-run，打印将要执行的 pecofence-cli 动作。
4. 用户明确同意后，`deskagent apply --yes`。
5. 记下输出里的快照 id。用户不满意时 `pecofence-cli snapshot restore <id>` 回滚。

永远不要跳过第 2 步直接 --yes。这会改变用户桌面。

## 常见需求

- 「顺便让以后新装的也自动归位」→ `apply --yes --with-rules`，
  然后提醒用户打开 PecoFence 的「新条目自动归档」，或让 `deskagent watch` 常驻。
- 「有些不认识的图标没分类」→ `deskagent resolve`，把模型的判断念给用户，
  用户认可后再跑 `plan --llm`。
- 「这个分类不对」→ 不要改规则包。直接告诉用户可以在 PecoFence 里拖一次，
  或者用 `pecofence-cli item move <路径> --to <栅栏>`。

## 输出怎么读

- `plan` 的「留在桌面」是故意的，不是失败：规则没覆盖，或该组条目太少不值得单独开栅栏。
- `rules` 输出的「可沉淀成持久规则」是扩展名类判断。快捷方式类判断无法规则化，
  因为 PecoFence 的条件里没有公司名和目标路径。这部分只能靠 `watch` 守护。

## 不要做的事

- 不要在没有 --yes 的情况下声称已经整理好了。
- 不要把 portal 栅栏当目标：portal 背后是磁盘上的真实文件夹，往里搬会移动真实文件。
  deskagent 已经会跳过它们，但你手动调 pecofence-cli 时要自己注意。
- 不要用 --llm 时假设数据不出本机。本地模式（127.0.0.1）才不出；
  云端模式会把去标识化后的条目信息发出去，先问用户。
"#;

// ---------------------------------------------------------------- 对照实验

fn bench(
    dirs: &[PathBuf],
    input: Option<&Path>,
    labels_path: &Path,
    assume_accuracy: Option<f64>,
    json: bool,
) -> anyhow::Result<()> {
    let items = match input {
        Some(p) => load_json_items(p)?,
        None => collect(dirs)?,
    };
    let pack = RulePack::from_yaml(BUILTIN_RULES)?;
    let raw = std::fs::read_to_string(labels_path)?;
    let labels: Labels = serde_yaml::from_str(&raw)?;

    let labeled: Vec<Item> = items
        .iter()
        .filter(|i| labels.labels.contains_key(&i.file_name))
        .cloned()
        .collect();
    let n = labeled.len();
    if n == 0 {
        anyhow::bail!("标注文件里没有一条能对上样本，检查键是否用了文件名");
    }

    // ---- 手写规则侧
    //
    // 不能只数「规则条数」：PecoFence 的条件里没有公司名、没有快捷方式目标路径，
    // 而快捷方式的分类恰好全靠这两样。所以要把条目分成两拨：
    //   能写成规则的 → 成本是规则条数（一条规则可覆盖多条同扩展名条目）
    //   写不成规则的 → 只能人工拖一次，一条算一次操作
    let mut ext_groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for item in &labeled {
        if deskagent_compile::rules::is_semantic_ext(&item.ext) {
            continue;
        }
        if let Some((_, group, _)) = pack.classify_with_rule(item) {
            ext_groups.entry(item.ext.clone()).or_default().insert(group);
        }
    }
    // 只保留「一个扩展名只对应一个分组」的，其余本身不携带分类信息
    let expressible: BTreeSet<String> = ext_groups
        .iter()
        .filter(|(_, gs)| gs.len() == 1)
        .map(|(e, _)| e.clone())
        .collect();
    let rule_groups: BTreeSet<String> = ext_groups
        .iter()
        .filter(|(_, gs)| gs.len() == 1)
        .flat_map(|(_, gs)| gs.iter().cloned())
        .collect();

    let mut rule_covered = 0usize;
    let mut rule_correct = 0usize;
    let mut manual = 0usize;
    for item in &labeled {
        let by_rule = expressible.contains(&item.ext);
        if !by_rule {
            // 规则表达不了，用户只能自己拖
            manual += 1;
            continue;
        }
        if let Some((_, group, _)) = pack.classify_with_rule(item) {
            rule_covered += 1;
            if labels.labels[&item.file_name] == group {
                rule_correct += 1;
            }
        }
    }
    // 每条规则一次 `rule add`，加上每条只能手拖的条目
    let rule_ops = (rule_groups.len() + manual) as f64;

    // ---- 模型侧：一次确认 + 需要人工纠正的条目数
    let (ai_ops, ai_accuracy, source) = if let Some(p) = assume_accuracy {
        if !(0.0..=1.0).contains(&p) {
            anyhow::bail!("--assume-accuracy 应在 0 到 1 之间");
        }
        (1.0 + n as f64 * (1.0 - p), p, "假设值".to_string())
    } else {
        let cfg = llm::Config::from_env().ok_or_else(|| {
            anyhow::anyhow!(
                "需要 --assume-accuracy <P> 估算，或配置模型实测（DESKAGENT_LLM_BASE_URL）"
            )
        })?;
        let client = llm::Client::new(cfg.clone());
        let suggestions = client.classify(&labeled, &candidate_groups())?;
        let mut wrong = 0usize;
        for item in &labeled {
            let got = suggestions
                .iter()
                .find(|s| s.file_name == item.file_name)
                .and_then(|s| s.group.clone());
            if got.as_deref() != Some(labels.labels[&item.file_name].as_str()) {
                wrong += 1;
            }
        }
        let acc = ratio(n - wrong, n);
        (
            1.0 + wrong as f64,
            acc,
            format!("实测，{}", cfg.describe_mode()),
        )
    };

    let threshold = rule_ops * 0.5;
    let passed = ai_ops <= threshold;

    if json {
        let payload = serde_json::json!({
            "sample": n,
            "rule_side": {
                "operations": rule_ops,
                "rule_count": rule_groups.len(),
                "rule_covered": rule_covered,
                "manual_drags": manual,
                "accuracy": ratio(rule_correct, rule_covered),
            },
            "ai_side": {
                "operations": ai_ops,
                "accuracy": ai_accuracy,
                "source": source,
            },
            "threshold": threshold,
            "passed": passed,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("样本 {n} 条（带标注）");
    println!("{}", "-".repeat(76));
    println!("手写规则侧");
    if rule_covered == 0 {
        println!("    能写成规则：0 条 —— PecoFence 的条件表达不了这些条目");
    } else {
        println!(
            "    能写成规则：{rule_covered} 条，需要 {} 条规则，准确率 {}%",
            rule_groups.len(),
            pct(rule_correct, rule_covered)
        );
    }
    println!("    写不成规则：{manual} 条（快捷方式等，PecoFence 条件表达不了，只能手拖）");
    println!("    成本 = {} 条规则 + {manual} 次拖拽 = {rule_ops} 次操作", rule_groups.len());
    println!("模型侧");
    println!("    准确率 {}%（{source}）", (ai_accuracy * 100.0).round());
    println!("    成本 = {ai_ops:.1} 次操作（1 次确认 + 每条判错/判不出的纠正）");
    println!("{}", "-".repeat(76));
    println!(
        "判定线：模型侧 ≤ 手写规则侧 × 50% = {threshold:.1}；实际 {ai_ops:.1} → {}",
        if passed { "达标" } else { "未达标" }
    );
    if rule_ops > 0.0 {
        // 反推：模型准确率掉到多少就不划算了
        let need = 1.0 - (threshold - 1.0) / n as f64;
        if need > 0.0 {
            println!("临界点：模型准确率需高于 {}%，低于这个数就不如手写规则", (need * 100.0).round());
        }
    }
    println!();
    println!("口径说明：");
    println!("    规则侧把「PecoFence 写不出来的判断」折成人工拖拽，一条算一次操作。");
    println!("    这一步不能省：只数规则条数会严重低估成本，因为快捷方式的分类全靠");
    println!("    目标路径和公司名，而 PecoFence 的条件里没有这两个。");
    println!("    模型侧把每条判错或判不出的条目都算一次人工纠正。");
    println!("    这个口径只看人工操作次数，不看时间，也不看规则写错后的返工成本。");
    Ok(())
}

// ---------------------------------------------------------------- 评测

#[derive(serde::Deserialize)]
struct Labels {
    labels: BTreeMap<String, String>,
}

/// 切分归属：FNV-1a 取模，跨平台跨版本稳定。
/// dev 参与调优，test 只允许看汇总数字，不允许逐条看错误。
fn is_dev(key: &str) -> bool {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h % 100 < 70
}

fn eval(
    items: &[Item],
    labels_path: &Path,
    rules_path: Option<&Path>,
    split: &str,
    json: bool,
) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(labels_path)?;
    let labels: Labels = serde_yaml::from_str(&raw)?;

    let rules_text = match rules_path {
        Some(p) => std::fs::read_to_string(p)?,
        None => BUILTIN_RULES.to_string(),
    };
    let pack = RulePack::from_yaml(&rules_text)?;

    let unlabeled: Vec<&str> = items
        .iter()
        .map(|i| i.file_name.as_str())
        .filter(|f| !labels.labels.contains_key(*f))
        .collect();

    let mut n_labeled = 0usize;
    let mut n_classified = 0usize;
    let mut n_correct = 0usize;
    // (人工分组, 规则分组) -> 次数
    let mut confusion: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut errors: Vec<(String, String, String, String)> = Vec::new();
    let mut missed: Vec<String> = Vec::new();

    for item in items {
        let Some(truth) = labels.labels.get(&item.file_name) else {
            continue;
        };
        let dev = is_dev(&item.file_name);
        match split {
            "dev" if !dev => continue,
            "test" if dev => continue,
            "all" | "dev" | "test" => {}
            other => anyhow::bail!("未知切分 {other}，应为 dev / test / all"),
        }

        n_labeled += 1;
        match pack.classify(item) {
            Some((group, reason)) => {
                n_classified += 1;
                *confusion
                    .entry((truth.clone(), group.clone()))
                    .or_default() += 1;
                if &group == truth {
                    n_correct += 1;
                } else {
                    errors.push((
                        item.file_name.clone(),
                        truth.clone(),
                        group,
                        reason,
                    ));
                }
            }
            None => {
                missed.push(item.file_name.clone());
                *confusion
                    .entry((truth.clone(), "（未分类）".to_string()))
                    .or_default() += 1;
            }
        }
    }

    if json {
        let payload = serde_json::json!({
            "split": split,
            "labeled": n_labeled,
            "classified": n_classified,
            "correct": n_correct,
            "coverage": ratio(n_classified, n_labeled),
            "accuracy": ratio(n_correct, n_classified),
            "errors": errors.iter().map(|(f, t, g, r)| serde_json::json!({
                "file": f, "truth": t, "predicted": g, "reason": r,
            })).collect::<Vec<_>>(),
            "missed": missed,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!(
        "SPLIT={}  LABELED={}  COVERAGE={}% ({}/{})  ACCURACY={}% ({}/{})",
        split,
        n_labeled,
        pct(n_classified, n_labeled),
        n_classified,
        n_labeled,
        pct(n_correct, n_classified),
        n_correct,
        n_classified,
    );
    if !unlabeled.is_empty() {
        println!("样本中缺标注 {} 条（不计入指标）", unlabeled.len());
    }

    // 只看 test 时，禁止暴露逐条错误，防止拿测试集调规则。
    if split == "test" {
        println!("{}", "-".repeat(76));
        println!("测试集逐条结果已隐藏，避免规则对测试集过拟合。");
        return Ok(());
    }

    if !errors.is_empty() {
        println!("{}", "-".repeat(76));
        println!("误分({})", errors.len());
        for (f, t, g, r) in &errors {
            println!("    {f:<28} 应为 {t:<6} 判为 {g:<6} ({r})");
        }
    }
    if !missed.is_empty() {
        println!("{}", "-".repeat(76));
        println!("漏分({})", missed.len());
        for f in &missed {
            println!("    {f}");
        }
    }
    Ok(())
}

fn ratio(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        n as f64 / d as f64
    }
}

// ---------------------------------------------------------------- 采集

/// 扫描目录并补齐 PE 版本信息。
fn collect(dirs: &[PathBuf]) -> anyhow::Result<Vec<Item>> {
    let targets: Vec<PathBuf> = if dirs.is_empty() {
        vec![desktop_dir()]
    } else {
        dirs.to_vec()
    };

    let mut items = Vec::new();
    let mut seen_paths = BTreeSet::new();

    for dir in &targets {
        if !dir.is_dir() {
            anyhow::bail!("目录不存在: {}", dir.display());
        }
        for path in walk(dir)? {
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            if let Some(item) = to_item(&path) {
                items.push(item);
            }
        }
    }

    items.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    Ok(items)
}

/// 递归枚举目录下的文件。
fn walk(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            // 开始菜单里有指向已卸载程序的失效链接，读不动就跳过。
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// 单个文件转成条目。
///
/// 不只处理快捷方式：真实桌面上一定有 PDF、图片、压缩包。早先只收 `.lnk`/`.url`，
/// 结果 `rules` 在只有快捷方式的机器上永远推不出扩展名规则，掩盖了这条通路。
fn to_item(path: &Path) -> Option<Item> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // shell 的目录元数据文件，不是用户内容
    if file_name.eq_ignore_ascii_case("desktop.ini") {
        return None;
    }

    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let name = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // 目标路径的含义：快捷方式指它指向的文件，普通文件指它自己
    let target = match ext.as_str() {
        ".lnk" => deskagent_scan::parse_file(path)
            .map(|i| i.target)
            .unwrap_or_default(),
        ".url" => read_url_target(path).unwrap_or_default(),
        _ => path.to_string_lossy().into_owned(),
    };

    // LNK 里的路径常带 `..`（如 `C:\Program Files\bilibili\..\Program Files\...`），
    // 不规范化会导致文件探测失败、读不到版本信息。
    let resolved = normalize(&target);
    let pe_source = match ext.as_str() {
        ".lnk" => resolved
            .as_deref()
            .filter(|p| Path::new(p).is_file())
            .map(PathBuf::from),
        // 可执行文件本身带版本信息
        ".exe" | ".dll" => Some(path.to_path_buf()),
        _ => None,
    };
    let pe = pe_source.and_then(|p| {
        let s = p.to_string_lossy().into_owned();
        deskagent_scan::read_version(&s)
    });

    Some(Item {
        name,
        file_name,
        path: path.to_string_lossy().into_owned(),
        target: resolved.unwrap_or(target),
        company: pe.as_ref().map(|p| p.company.clone()).unwrap_or_default(),
        product: pe.as_ref().map(|p| p.product.clone()).unwrap_or_default(),
        ext,
    })
}

fn read_url_target(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        if line.len() > 4 && line[..4].eq_ignore_ascii_case("URL=") {
            return Some(line[4..].trim().to_string());
        }
    }
    None
}

/// 消掉路径里的 `..`，并去掉 canonicalize 加上的 `\\?\` 前缀。
fn normalize(target: &str) -> Option<String> {
    if target.is_empty() || !target.contains('\\') {
        return None;
    }
    let canon = Path::new(target).canonicalize().ok()?;
    let s = canon.to_string_lossy().into_owned();
    Some(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

/// 桌面路径。OneDrive 重定向后位置会变，这里按常见顺序探测。
fn desktop_dir() -> PathBuf {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        for sub in ["Desktop", "OneDrive\\Desktop", "OneDrive\\桌面"] {
            let d = PathBuf::from(&profile).join(sub);
            if d.is_dir() {
                return d;
            }
        }
    }
    PathBuf::from(".")
}

fn pct(n: usize, total: usize) -> usize {
    if total == 0 {
        0
    } else {
        n * 100 / total
    }
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}
