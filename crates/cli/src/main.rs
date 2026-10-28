use std::collections::BTreeMap;
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
    /// 扫描桌面，输出条目清单
    Scan {
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 扫描并分类，输出分组结果
    Classify {
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Scan { json } => scan(json),
        Cmd::Classify { json } => classify(json),
    }
}

fn scan(json: bool) -> anyhow::Result<()> {
    let items = collect()?;

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

fn classify(json: bool) -> anyhow::Result<()> {
    let items = collect()?;
    let pack = RulePack::from_yaml(BUILTIN_RULES)?;

    let mut groups: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut unclassified = Vec::new();

    for item in &items {
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

/// 扫描桌面并补齐 PE 版本信息。
fn collect() -> anyhow::Result<Vec<Item>> {
    let dir = desktop_dir();
    if !dir.is_dir() {
        anyhow::bail!("桌面目录不存在: {}", dir.display());
    }

    let mut items = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        if ext != ".lnk" && ext != ".url" {
            continue;
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
            deskagent_scan::parse_file(&path)
                .map(|i| i.target)
                .unwrap_or_default()
        } else {
            read_url_target(&path).unwrap_or_default()
        };

        // LNK 里的路径常带 `..`（如 `C:\Program Files\bilibili\..\Program Files\...`），
        // 不规范化会导致文件探测失败、读不到版本信息。
        let resolved = normalize(&target);
        let pe = resolved
            .as_deref()
            .filter(|p| Path::new(p).is_file())
            .and_then(deskagent_scan::read_version);

        items.push(Item {
            name,
            file_name,
            path: path.to_string_lossy().into_owned(),
            target: resolved.unwrap_or(target),
            company: pe.as_ref().map(|p| p.company.clone()).unwrap_or_default(),
            product: pe.as_ref().map(|p| p.product.clone()).unwrap_or_default(),
            ext,
        });
    }

    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(items)
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
