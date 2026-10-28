//! `pecofence-cli` 适配层。
//!
//! 只做三件事：找到可执行文件、拼出参数、把退出码翻译成可读错误。
//! 参数拼装全部抽成纯函数，便于断言——这里错了整个产品就错了，但错误不会在
//! 本机暴露（本机没装 PecoFence，跑不通）。
//!
//! 契约来源：PecoFence `docs/CLI.md` 与 `crates/ipc/src/dto.rs`。
//! 要点：
//!   - 每次调用开一条命名管道连接，发一行 JSON、收一行 JSON，无常驻进程。
//!   - 退出码 0 成功 / 1 应用报错 / 2 用法错误 / 3 应用没运行 / 4 超时。
//!   - 成功时 stdout 只有 JSON；失败时 stderr 是 `{"error":{...}}`。
//!   - 传 `--ascii` 让非 ASCII 字符全部转成 `\uXXXX`，规避 Windows 代码页问题。
//!   - 所有全局标志在 clap 里都是 `global = true`，放在子命令前后都行。
//!
//! 已核对的输出形状（本机没装 PecoFence，只能对源码核，不能实测）：
//!   - `monitor list` → 数组，元素 `{id,label,rect,workArea,dpi,primary}`，
//!     `Rect` 是 `{x,y,w,h}` 对象，不是数组
//!   - `fence list`   → 裸数组（`run.rs` 里就是 `list.as_array()`），不是 `{list:[...]}`
//!   - `snapshot save` → `{changed, snapshot:{id,name,ts,fenceCount}}`
//!   - `item move`    → `{changed, ...}`，跳过项放在 `skipped` 并附 `warning`

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("找不到 pecofence-cli。请先安装 PecoFence，或用环境变量 PECOFENCE_CLI 指定可执行文件路径")]
    NotFound,

    #[error("PecoFence 没有在运行。请先启动它，再重试")]
    NotRunning,

    #[error("pecofence-cli 用法错误：{0}")]
    Usage(String),

    #[error("pecofence-cli 超时：{0}\n命令可能已经执行，重试前先用 fence list 读回状态")]
    Timeout(String),

    #[error("PecoFence 返回 {code}：{message}")]
    App { code: i32, message: String },

    #[error("无法解析 pecofence-cli 的输出：{0}")]
    Decode(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// 全局标志：强制 ASCII 输出、单行 JSON。
const GLOBAL_FLAGS: [&str; 2] = ["--ascii", "--compact"];

pub struct Cli {
    path: PathBuf,
}

impl Cli {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 查找顺序：环境变量 `PECOFENCE_CLI` → `PATH`。
    pub fn discover() -> Result<Self> {
        if let Ok(p) = std::env::var("PECOFENCE_CLI") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Ok(Self::new(p));
            }
        }

        let exe = if cfg!(windows) {
            "pecofence-cli.exe"
        } else {
            "pecofence-cli"
        };
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let cand = dir.join(exe);
                if cand.is_file() {
                    return Ok(Self::new(cand));
                }
            }
        }
        Err(Error::NotFound)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 执行一次调用，返回解析后的 JSON。
    pub fn call(&self, args: &[String]) -> Result<serde_json::Value> {
        let mut full: Vec<String> = GLOBAL_FLAGS.iter().map(|s| s.to_string()).collect();
        full.extend_from_slice(args);

        let out = Command::new(&self.path).args(&full).output()?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

        match out.status.code() {
            Some(0) => {
                if stdout.trim().is_empty() {
                    return Ok(serde_json::Value::Null);
                }
                serde_json::from_str(&stdout)
                    .map_err(|e| Error::Decode(format!("{e}: {}", truncate(&stdout, 200))))
            }
            Some(3) => Err(Error::NotRunning),
            Some(2) => Err(Error::Usage(message_of(&stderr).unwrap_or(stderr))),
            Some(4) => Err(Error::Timeout(message_of(&stderr).unwrap_or(stderr))),
            Some(code) => Err(Error::App {
                code,
                message: message_of(&stderr).unwrap_or_else(|| stderr.clone()),
            }),
            None => Err(Error::Decode("进程被信号终止".into())),
        }
    }

    pub fn status(&self) -> Result<serde_json::Value> {
        self.call(&cmd_status())
    }

    pub fn list_fences(&self) -> Result<serde_json::Value> {
        self.call(&cmd_fence_list())
    }

    pub fn list_monitors(&self) -> Result<serde_json::Value> {
        self.call(&cmd_monitor_list())
    }

    pub fn snapshot_save(&self, name: &str) -> Result<serde_json::Value> {
        self.call(&cmd_snapshot_save(name))
    }

    pub fn create_fence_rect(&self, title: &str, rect: &str) -> Result<serde_json::Value> {
        self.call(&cmd_fence_create_rect(title, rect))
    }

    pub fn create_fence_below(&self, title: &str, below: &str) -> Result<serde_json::Value> {
        self.call(&cmd_fence_create_below(title, below))
    }

    pub fn fit_fence(&self, fence: &str) -> Result<serde_json::Value> {
        self.call(&cmd_fence_fit(fence))
    }

    pub fn move_items(&self, paths: &[String], to: &str, dry_run: bool) -> Result<serde_json::Value> {
        self.call(&cmd_item_move(paths, to, dry_run))
    }

    pub fn add_ext_rule(&self, name: &str, to: &str, exts: &[String]) -> Result<serde_json::Value> {
        self.call(&cmd_rule_add_ext(name, to, exts))
    }
}

// ---------------------------------------------------------------- 参数拼装

pub fn cmd_status() -> Vec<String> {
    vec!["status".into()]
}

pub fn cmd_monitor_list() -> Vec<String> {
    vec!["monitor".into(), "list".into()]
}

pub fn cmd_fence_list() -> Vec<String> {
    vec!["fence".into(), "list".into()]
}

pub fn cmd_snapshot_save(name: &str) -> Vec<String> {
    vec!["snapshot".into(), "save".into(), name.into()]
}

/// 第一个栅栏需要绝对坐标，后续栅栏用 `--below` 相对排布。
pub fn cmd_fence_create_rect(title: &str, rect: &str) -> Vec<String> {
    vec![
        "fence".into(),
        "create".into(),
        "--title".into(),
        title.into(),
        "--rect".into(),
        rect.into(),
    ]
}

pub fn cmd_fence_create_below(title: &str, below: &str) -> Vec<String> {
    vec![
        "fence".into(),
        "create".into(),
        "--title".into(),
        title.into(),
        "--below".into(),
        below.into(),
    ]
}

pub fn cmd_fence_fit(fence: &str) -> Vec<String> {
    vec!["fence".into(), "fit".into(), fence.into()]
}

/// `item move` 的 ITEM 支持 id、路径、名称。我们用完整路径，最不容易撞名。
pub fn cmd_item_move(paths: &[String], to: &str, dry_run: bool) -> Vec<String> {
    let mut v = vec!["item".into(), "move".into()];
    v.extend(paths.iter().cloned());
    v.push("--to".into());
    v.push(to.into());
    if dry_run {
        v.push("--dry-run".into());
    }
    v
}

/// PecoFence 的规则条件只认扩展名/类型/文件名等属性，没有公司名和目标路径，
/// 所以只有「扩展名 → 分组」这类判断能沉淀成持久规则，其余只能一次性移动。
pub fn cmd_rule_add_ext(name: &str, to: &str, exts: &[String]) -> Vec<String> {
    vec![
        "rule".into(),
        "add".into(),
        "--name".into(),
        name.into(),
        "--ext".into(),
        exts.join(","),
        "--to".into(),
        to.into(),
    ]
}

// ---------------------------------------------------------------- 辅助

/// 从 `fence list` 的返回值里挑出**可以安全复用**的栅栏标题。
///
/// 两类必须排除：
///   - `kind == "portal"`：portal 背后是磁盘上的真实文件夹，`item move` 到 portal
///     等于在资源管理器里搬文件，会真的改动用户文件。这是数据安全问题，不是体验问题。
///   - 带 `tabHost` 的：它是寄宿在别的栅栏里的标签页，几何和成员关系都归宿主管。
pub fn reusable_titles(list: &serde_json::Value) -> Vec<String> {
    let Some(arr) = list.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter(|f| f.get("kind").and_then(|k| k.as_str()) != Some("portal"))
        .filter(|f| !f.get("tabHost").map(|h| h.is_string()).unwrap_or(false))
        .filter_map(|f| f.get("title").and_then(|t| t.as_str()).map(str::to_string))
        .collect()
}

/// 列出全部栅栏标题，含 portal 和标签页。
/// 用来发现「标题撞车但不能复用」的情况：此时再创建一个同名栅栏，
/// 后续 `--to <标题>` 会变成 `ambiguous_fence`。
pub fn all_titles(list: &serde_json::Value) -> Vec<String> {
    list.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|f| f.get("title").and_then(|t| t.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// stderr 形如 `{"error":{"code","message","hint"?}}`，取出可读消息。
fn message_of(stderr: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).ok()?;
    let err = v.get("error")?;
    let code = err.get("code").and_then(|c| c.as_str()).unwrap_or("");
    let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
    let hint = err.get("hint").and_then(|h| h.as_str()).unwrap_or("");
    let mut s = String::new();
    if !code.is_empty() {
        s.push_str(&format!("[{code}] "));
    }
    s.push_str(msg);
    if !hint.is_empty() {
        s.push_str(&format!("（{hint}）"));
    }
    Some(s.trim().to_string())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_rect_has_title_and_rect() {
        let a = cmd_fence_create_rect("游戏", "100,100,480,360");
        assert_eq!(a[0], "fence");
        assert_eq!(a[1], "create");
        assert_eq!(a[3], "游戏");
        assert_eq!(a[4], "--rect");
        assert_eq!(a[5], "100,100,480,360");
    }

    #[test]
    fn create_below_uses_relative_placement() {
        let a = cmd_fence_create_below("开发", "游戏");
        assert_eq!(a[4], "--below");
        assert_eq!(a[5], "游戏");
    }

    /// 路径含空格时必须是单个 argv 元素，不能拼成字符串。
    #[test]
    fn move_keeps_paths_as_separate_args() {
        let paths = vec![
            r"C:\Program Files\Zed\Zed.exe".to_string(),
            r"C:\Users\me\Desktop\Steam.lnk".to_string(),
        ];
        let a = cmd_item_move(&paths, "开发", false);
        assert_eq!(a[0], "item");
        assert_eq!(a[1], "move");
        assert_eq!(a[2], r"C:\Program Files\Zed\Zed.exe");
        assert_eq!(a[3], r"C:\Users\me\Desktop\Steam.lnk");
        assert_eq!(a[4], "--to");
        assert_eq!(a[5], "开发");
        assert!(!a.contains(&"--dry-run".to_string()));
    }

    #[test]
    fn dry_run_appends_flag_last() {
        let a = cmd_item_move(&["a.lnk".to_string()], "工具", true);
        assert_eq!(a.last().unwrap(), "--dry-run");
    }

    #[test]
    fn ext_rule_joins_with_comma() {
        let exts = vec!["pdf".to_string(), "docx".to_string()];
        let a = cmd_rule_add_ext("文档", "办公", &exts);
        assert_eq!(a[5], "pdf,docx");
    }

    #[test]
    fn error_message_extracts_code_and_hint() {
        let s = r#"{"error":{"code":"fence_not_found","message":"no fence","hint":"fence list"}}"#;
        let m = message_of(s).unwrap();
        assert!(m.contains("fence_not_found"));
        assert!(m.contains("no fence"));
        assert!(m.contains("fence list"));
    }

    #[test]
    fn error_message_tolerates_non_json() {
        assert!(message_of("boom").is_none());
    }

    #[test]
    fn global_flags_precede_subcommand() {
        // 与 call() 的拼接顺序保持一致
        let mut full: Vec<String> = GLOBAL_FLAGS.iter().map(|s| s.to_string()).collect();
        full.extend(cmd_status());
        assert_eq!(full, vec!["--ascii", "--compact", "status"]);
    }

    #[test]
    fn reusable_titles_skips_portals() {
        // portal 背后是真实文件夹，往里 move 会移动真实文件
        let v = serde_json::json!([
            {"id": "a", "title": "游戏", "kind": "virtual"},
            {"id": "b", "title": "工具", "kind": "portal", "portal": {"path": "D:\\Tools"}},
            {"id": "c", "title": "inbox", "kind": "inbox"}
        ]);
        assert_eq!(reusable_titles(&v), vec!["游戏", "inbox"]);
    }

    #[test]
    fn reusable_titles_skips_hosted_tabs() {
        let v = serde_json::json!([
            {"id": "a", "title": "工作", "kind": "virtual"},
            {"id": "b", "title": "子页", "kind": "virtual", "tabHost": "a"}
        ]);
        assert_eq!(reusable_titles(&v), vec!["工作"]);
    }

    #[test]
    fn reusable_titles_tolerates_unexpected_shape() {
        // fence list 不是数组时不要 panic，也不要误判成「全都可复用」
        assert!(reusable_titles(&serde_json::json!({"list": []})).is_empty());
        assert!(reusable_titles(&serde_json::Value::Null).is_empty());
    }
}
