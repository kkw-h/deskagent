//! PecoFence 客户端：直接走命名管道与运行中的应用通信。
//!
//! 合并源码之后这里不再启动 `pecofence-cli` 子进程，而是直接调用
//! [`pecofence_ipc::client`]，也就是 `pecofence-cli` 自己用的那条通路。
//! 这样做换来三件事：
//!   - 不用在 PATH 里找可执行文件，也不用管它的版本；
//!   - 参数是 `Method` 枚举，拼错在编译期就暴露，不再靠字符串对齐；
//!   - 少一次进程启动，`watch` 这种高频路径的延迟明显下降。
//!
//! 对外的 API 保持原样（返回 `serde_json::Value`），上层 deskagent 不必跟着改。
//!
//! ## 两条安全约定（合并前就存在，继续保留）
//!
//! - `reusable_titles` 会排除 portal 栅栏：portal 背后是磁盘上的真实文件夹，
//!   往里 `items.move` 等于在资源管理器里搬文件。
//! - 带 `tabHost` 的标签页也不复用：它的几何和成员关系都归宿主管。

use pecofence_ipc::client;
use pecofence_ipc::{ErrorCode, IpcError, Method, Rect, Response};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("PecoFence 没有在运行。请先启动它，再重试")]
    NotRunning,

    #[error("PecoFence 超时：{0}\n命令可能已经执行，重试前先用 fence list 读回状态")]
    Timeout(String),

    #[error("请求不合法：{0}")]
    Usage(String),

    #[error("PecoFence 返回 [{code}]：{message}")]
    App { code: String, message: String },

    #[error("无法解析 PecoFence 的回复：{0}")]
    Decode(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<IpcError> for Error {
    fn from(e: IpcError) -> Self {
        match e.code {
            ErrorCode::NotRunning => Error::NotRunning,
            ErrorCode::Timeout => Error::Timeout(e.message),
            ErrorCode::Usage | ErrorCode::VersionMismatch => Error::Usage(e.message),
            other => Error::App {
                code: format!("{other:?}"),
                message: e.message,
            },
        }
    }
}

/// 与运行中的应用通信的句柄。
///
/// `instance` 对应 `--instance` / `PECOFENCE_INSTANCE`，用于连测试实例；
/// 正常情况留空即可。
#[derive(Debug, Clone)]
pub struct Cli {
    instance: Option<String>,
    timeout_ms: u32,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            instance: None,
            timeout_ms: pecofence_ipc::DEFAULT_TIMEOUT_MS,
        }
    }
}

impl Cli {
    pub fn new(instance: Option<String>) -> Self {
        Self {
            instance,
            ..Self::default()
        }
    }

    /// 从环境变量读实例名，再探测一次 `status` 确认应用真的在跑。
    ///
    /// 合并后没有「找不到可执行文件」这种失败模式了：要么连上应用，要么应用没启动。
    pub fn discover() -> Result<Self> {
        let instance = std::env::var("PECOFENCE_INSTANCE")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let cli = Self::new(instance);
        // 探测一次，把「没在运行」尽早暴露出来，而不是等到第一次真正修改时才失败
        cli.status()?;
        Ok(cli)
    }

    /// 把超时调大，`watch` 这类需要长时间等待的场景要用。
    pub fn with_timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    pub fn instance(&self) -> Option<&str> {
        self.instance.as_deref()
    }

    /// 发一条请求，返回 `result` 字段；`warning` 会被并进返回对象。
    pub fn call(&self, method: Method) -> Result<Value> {
        let resp = client::send(self.instance.as_deref(), method, self.timeout_ms)?;
        payload(resp)
    }

    pub fn status(&self) -> Result<Value> {
        self.call(Method::StatusGet)
    }

    pub fn list_fences(&self) -> Result<Value> {
        self.call(Method::FencesList)
    }

    pub fn list_monitors(&self) -> Result<Value> {
        self.call(Method::MonitorsList)
    }

    pub fn snapshot_save(&self, name: &str) -> Result<Value> {
        self.call(Method::SnapshotsSave {
            name: name.to_string(),
        })
    }

    /// `rect` 是 `"x,y,w,h"` 形式（物理像素，虚拟屏坐标）。
    pub fn create_fence_rect(&self, title: &str, rect: &str) -> Result<Value> {
        let rect = parse_rect(rect).ok_or_else(|| Error::Usage(format!("矩形格式错误：{rect}")))?;
        self.call(Method::FencesCreate {
            title: Some(title.to_string()),
            rect: Some(rect),
            monitor: None,
            portal: None,
        })
    }

    pub fn fit_fence(&self, fence: &str) -> Result<Value> {
        self.call(Method::FencesFit {
            fence: fence.to_string(),
        })
    }

    /// 把条目移进栅栏。`dry_run` 走 `items.planMove`：服务端不会执行，
    /// 只会算出会怎么动——这是单独的方法而不是一个标志位，老版本应用会答
    /// `unknown method`，而不是忽略标志真的去移动。
    pub fn move_items(&self, paths: &[String], to: &str, dry_run: bool) -> Result<Value> {
        let method = if dry_run {
            Method::ItemsPlanMove {
                items: paths.to_vec(),
                to: to.to_string(),
            }
        } else {
            Method::ItemsMove {
                items: paths.to_vec(),
                to: to.to_string(),
            }
        };
        self.call(method)
    }

    /// 加一条扩展名规则。
    ///
    /// 走的是完整的 `rules.add`（`all_of` 里放一个 `Cond::Ext`），
    /// 不是简化的 `rule add --ext`：合并后不必再迁就命令行的简写形式。
    pub fn add_ext_rule(&self, name: &str, to: &str, exts: &[String]) -> Result<Value> {
        let cleaned: Vec<String> = exts
            .iter()
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        if cleaned.is_empty() {
            return Err(Error::Usage("扩展名列表为空".into()));
        }
        self.call(Method::RulesAdd {
            name: name.to_string(),
            target: to.to_string(),
            all_of: vec![pecofence_core::rules::Cond::Ext(cleaned)],
            index: None,
        })
    }

    /// 用完整条件加一条规则。
    pub fn add_rule(
        &self,
        name: &str,
        to: &str,
        all_of: Vec<pecofence_core::rules::Cond>,
    ) -> Result<Value> {
        if all_of.is_empty() {
            return Err(Error::Usage("规则至少需要一个条件".into()));
        }
        self.call(Method::RulesAdd {
            name: name.to_string(),
            target: to.to_string(),
            all_of,
            index: None,
        })
    }

    pub fn list_rules(&self) -> Result<Value> {
        self.call(Method::RulesGet)
    }

    /// 阻塞到桌面上出现一个新条目，返回该事件。
    ///
    /// 走 `events.subscribe` 长连接：第一条回复是订阅确认，之后是事件流。
    /// 心跳事件会被跳过。超时给得比普通调用长，因为它本来就要等。
    pub fn watch_inbox_once(&self) -> Result<Value> {
        let method = Method::EventsSubscribe {
            fence: Some("inbox".to_string()),
            events: Some(vec!["item.added".to_string()]),
        };
        let mut captured: Option<Value> = None;
        let finished = client::stream(self.instance.as_deref(), method, self.timeout_ms, |line| {
            if !line.ok {
                return false;
            }
            let Some(result) = line.result else {
                return true;
            };
            // 订阅确认没有 event 字段，跳过
            if result.get("event").and_then(|e| e.as_str()) == Some("item.added") {
                captured = Some(result);
                return false;
            }
            true
        })?;

        match (finished, captured) {
            (_, Some(event)) => Ok(event),
            (true, None) => Ok(Value::Null),
            (false, None) => Err(Error::Decode("事件流被意外关闭".into())),
        }
    }
}

/// 把 `{ok, result, warning}` 摊平成调用方要的 JSON。
fn payload(resp: Response) -> Result<Value> {
    if !resp.ok {
        let err = resp
            .error
            .unwrap_or_else(|| IpcError::internal("PecoFence 返回了失败状态但没有给出错误详情"));
        return Err(err.into());
    }
    Ok(match (resp.result, resp.warning) {
        (result, None) => result.unwrap_or(Value::Null),
        (Some(Value::Object(mut map)), Some(warning)) => {
            map.insert("warning".into(), Value::String(warning));
            Value::Object(map)
        }
        (result, Some(warning)) => serde_json::json!({
            "result": result.unwrap_or(Value::Null),
            "warning": warning,
        }),
    })
}

fn parse_rect(s: &str) -> Option<Rect> {
    let parts: Vec<i32> = s
        .split(',')
        .map(|p| p.trim().parse::<i32>().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() != 4 {
        return None;
    }
    Some(Rect {
        x: parts[0],
        y: parts[1],
        w: parts[2],
        h: parts[3],
    })
}

// ---------------------------------------------------------------- 辅助

/// 从 `fence list` 的返回值里挑出**可以安全复用**的栅栏标题。
///
/// 两类必须排除：
///   - `kind == "portal"`：portal 背后是磁盘上的真实文件夹，`items.move` 到 portal
///     等于在资源管理器里搬文件，会真的改动用户文件。这是数据安全问题，不是体验问题。
///   - 带 `tabHost` 的：它是寄宿在别的栅栏里的标签页，几何和成员关系都归宿主管。
pub fn reusable_titles(list: &Value) -> Vec<String> {
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
pub fn all_titles(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|f| f.get("title").and_then(|t| t.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_parses_four_numbers() {
        let r = parse_rect("100,100,480,360").unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (100, 100, 480, 360));
    }

    #[test]
    fn rect_rejects_wrong_shapes() {
        assert!(parse_rect("1,2,3").is_none());
        assert!(parse_rect("a,b,c,d").is_none());
        assert!(parse_rect("").is_none());
    }

    /// 负数坐标是合法的：多屏时主屏左边那台显示器就是负 x。
    #[test]
    fn rect_accepts_negative_origin() {
        let r = parse_rect("-1920,0,1920,1080").unwrap();
        assert_eq!(r.x, -1920);
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
        assert!(reusable_titles(&Value::Null).is_empty());
    }

    #[test]
    fn all_titles_includes_portals() {
        let v = serde_json::json!([
            {"title": "游戏", "kind": "virtual"},
            {"title": "工具", "kind": "portal"}
        ]);
        assert_eq!(all_titles(&v), vec!["游戏", "工具"]);
    }

    /// 成功回复里的 warning 要并进结果，调用方才能看到「命令生效但配置没写成功」这类情况。
    #[test]
    fn warning_is_merged_into_payload() {
        let mut resp = Response::ok(serde_json::json!({"changed": true}));
        resp.warning = Some("配置写入失败".into());
        let v = payload(resp).unwrap();
        assert_eq!(v["changed"], true);
        assert_eq!(v["warning"], "配置写入失败");
    }

    #[test]
    fn failed_reply_becomes_error() {
        let resp = Response::err(IpcError::new(ErrorCode::FenceNotFound, "没有这个栅栏"));
        let e = payload(resp).unwrap_err();
        match e {
            Error::App { code, message } => {
                assert_eq!(code, "FenceNotFound");
                assert!(message.contains("没有这个栅栏"));
            }
            other => panic!("期望 App 错误，得到 {other:?}"),
        }
    }

    /// 应用没启动时要映射成 NotRunning，上层靠这个变体给出「先启动它」的提示。
    #[test]
    fn not_running_maps_to_dedicated_variant() {
        let e: Error = IpcError::new(ErrorCode::NotRunning, "no pipe").into();
        assert!(matches!(e, Error::NotRunning));
    }

    #[test]
    fn timeout_maps_to_dedicated_variant() {
        let e: Error = IpcError::new(ErrorCode::Timeout, "no reply").into();
        assert!(matches!(e, Error::Timeout(_)));
    }
}
