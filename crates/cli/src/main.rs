use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use deskagent_classify::{Item, RulePack};

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
    }
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

/// 单个文件转成条目；非快捷方式返回 None。
fn to_item(path: &Path) -> Option<Item> {
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if ext != ".lnk" && ext != ".url" {
        return None;
    }

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let target = if ext == ".lnk" {
        deskagent_scan::parse_file(path)
            .map(|i| i.target)
            .unwrap_or_default()
    } else {
        read_url_target(path).unwrap_or_default()
    };

    // LNK 里的路径常带 `..`（如 `C:\Program Files\bilibili\..\Program Files\...`），
    // 不规范化会导致文件探测失败、读不到版本信息。
    let resolved = normalize(&target);
    let pe = resolved
        .as_deref()
        .filter(|p| Path::new(p).is_file())
        .and_then(deskagent_scan::read_version);

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
