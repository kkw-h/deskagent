//! LLM 分类客户端，双模式：云端与本地，都走 OpenAI 兼容接口。
//!
//! 为什么用 `curl` 子进程而不是 HTTP 库：
//! 本项目在 `x86_64-pc-windows-gnu` 上构建，没有 MSVC，也没有 dlltool。
//! 引入 TLS 栈（ring / aws-lc-rs）需要 C 编译器，会直接把构建打回原形。
//! Windows 10 1803 起自带 `curl.exe`（Schannel 后端），调它零依赖且证书链正确。
//!
//! 两种模式只是配置差异，没有代码分支：
//!   云端  DESKAGENT_LLM_BASE_URL=https://api.openai.com/v1  + API Key
//!   本地  DESKAGENT_LLM_BASE_URL=http://127.0.0.1:11434/v1 （Ollama，无需 Key）
//!
//! 隐私：上传内容一律经 [`privacy::RedactedItem`] 去标识化，路径只留产品段。

pub mod privacy;

use std::io::Write;
use std::process::{Command, Stdio};

use deskagent_classify::Item;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("没有配置模型。设置 DESKAGENT_LLM_BASE_URL / DESKAGENT_LLM_MODEL（云端还需要 DESKAGENT_LLM_API_KEY）")]
    NotConfigured,

    #[error("调用模型失败：{0}")]
    Http(String),

    #[error("模型返回的内容不是预期 JSON：{0}")]
    Decode(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Config {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub timeout_secs: u64,
}

impl Config {
    /// 从环境变量读取。一个变量都不设时返回 `None`，表示没启用模型。
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("DESKAGENT_LLM_BASE_URL").ok()?;
        if base_url.trim().is_empty() {
            return None;
        }
        Some(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: std::env::var("DESKAGENT_LLM_API_KEY")
                .ok()
                .filter(|k| !k.trim().is_empty()),
            model: std::env::var("DESKAGENT_LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            timeout_secs: std::env::var("DESKAGENT_LLM_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60),
        })
    }

    /// 本机回环地址视为本地模式。本地模式不需要 Key，也不该把数据算作外发。
    pub fn is_local(&self) -> bool {
        let u = &self.base_url;
        u.contains("127.0.0.1") || u.contains("localhost") || u.contains("[::1]")
    }

    pub fn describe_mode(&self) -> String {
        if self.is_local() {
            format!("本地（{}，模型 {}）", self.base_url, self.model)
        } else {
            format!("云端（{}，模型 {}）", self.base_url, self.model)
        }
    }
}

/// 模型给出的一个判断。
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// 对应 `Item::file_name`。
    pub file_name: String,
    /// `None` 表示模型也拿不准。
    pub group: Option<String>,
    pub reason: String,
}

pub struct Client {
    cfg: Config,
}

impl Client {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// 让模型给一批条目分组。只上传去标识化后的字段。
    pub fn classify(&self, items: &[Item], groups: &[String]) -> Result<Vec<Suggestion>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let redacted: Vec<privacy::RedactedItem> =
            items.iter().map(privacy::RedactedItem::from_item).collect();

        let system = format!(
            "你是桌面文件分类器。给定一批桌面条目和固定的候选分组，为每个条目选出最合适的分组。\n\
             规则：\n\
             1. group 必须严格取自候选列表，不要发明新分组。\n\
             2. 拿不准就把 group 设为 null，不要猜。\n\
             3. 只输出一个 JSON 对象，不要任何解释文字或 Markdown 代码块。\n\
             4. 输出格式：{{\"assignments\":[{{\"file\":\"<file_name>\",\"group\":\"<候选分组或null>\",\"reason\":\"<20字以内>\"}}]}}\n\
             候选分组：{}",
            groups.join("、")
        );

        let mut rows = Vec::new();
        for r in &redacted {
            rows.push(serde_json::json!({
                "file": format!("{}{}", r.name, r.ext),
                "name": r.name,
                "ext": r.ext,
                "company": r.company,
                "target": r.target_hint,
            }));
        }
        let user = serde_json::json!({ "items": rows }).to_string();

        let content = self.chat(&system, &user)?;
        parse_assignments(&content, &redacted, groups)
    }

    fn chat(&self, system: &str, user: &str) -> Result<String> {
        let body = serde_json::json!({
            "model": self.cfg.model,
            "temperature": 0,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        });
        let payload = serde_json::to_vec(&body).map_err(|e| Error::Decode(e.to_string()))?;

        let url = format!("{}/chat/completions", self.cfg.base_url);
        let timeout = self.cfg.timeout_secs.to_string();

        let mut cmd = Command::new(curl_exe());
        cmd.args([
            "-sS",
            "--fail-with-body",
            "--max-time",
            &timeout,
            "-X",
            "POST",
            &url,
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ]);
        if let Some(key) = &self.cfg.api_key {
            cmd.args(["-H", &format!("Authorization: Bearer {key}")]);
        }
        // 本地模型必须绕开代理：机器上一旦设了 http_proxy，curl 会把
        // 127.0.0.1 的请求也送进代理，直接 502。
        // 云端模型反过来要保留代理——访问境外 API 常常只能靠它。
        if self.cfg.is_local() {
            cmd.args(["--noproxy", "*"]);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| Error::Http("无法写入请求体".into()))?
            .write_all(&payload)?;
        let out = child.wait_with_output()?;

        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(Error::Http(format!(
                "curl 退出码 {:?}：{}",
                out.status.code(),
                truncate(
                    if stderr.trim().is_empty() {
                        &stdout
                    } else {
                        &stderr
                    },
                    300
                )
            )));
        }

        let v: serde_json::Value = serde_json::from_str(&stdout)
            .map_err(|e| Error::Decode(format!("{e}: {}", truncate(&stdout, 200))))?;
        v.get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                Error::Decode(format!(
                    "缺少 choices[0].message.content: {}",
                    truncate(&stdout, 200)
                ))
            })
    }
}

/// 从模型回复里抽出分组建议。
///
/// 容错点：模型常会套 ```json 围栏、或在 JSON 前后加一句话。
/// 另外分组名必须是候选之一，否则视为无效——宁可判不出来，也不要塞进不存在的分组。
fn parse_assignments(
    content: &str,
    items: &[privacy::RedactedItem],
    groups: &[String],
) -> Result<Vec<Suggestion>> {
    let json_text = extract_json_object(content)
        .ok_or_else(|| Error::Decode(format!("找不到 JSON 对象：{}", truncate(content, 200))))?;
    let v: serde_json::Value =
        serde_json::from_str(&json_text).map_err(|e| Error::Decode(format!("{e}")))?;

    let arr = v
        .get("assignments")
        .and_then(|a| a.as_array())
        .ok_or_else(|| Error::Decode("缺少 assignments 数组".into()))?;

    let known: std::collections::BTreeSet<&str> = groups.iter().map(|g| g.as_str()).collect();
    let mut out = Vec::new();

    for row in arr {
        let Some(file) = row.get("file").and_then(|f| f.as_str()) else {
            continue;
        };
        // 模型可能回 `Steam.lnk` 也可能回 `Steam`，两种都认
        let matched = items
            .iter()
            .find(|i| format!("{}{}", i.name, i.ext) == file || i.name == file)
            .map(|i| format!("{}{}", i.name, i.ext));
        let Some(file_name) = matched else {
            continue;
        };

        let group = row
            .get("group")
            .and_then(|g| g.as_str())
            .map(str::trim)
            .filter(|g| !g.is_empty())
            .filter(|g| known.contains(g))
            .map(str::to_string);

        out.push(Suggestion {
            file_name,
            group,
            reason: row
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .chars()
                .take(60)
                .collect(),
        });
    }

    Ok(out)
}

/// 找出第一个能完整解析的 JSON 对象，容忍前后的说明文字与代码围栏。
fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    // 从后往前找右花括号，取第一个能解析的
    let bytes: Vec<char> = s.chars().collect();
    let start_char = s[..start].chars().count();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for i in start_char..bytes.len() {
        let c = bytes[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let candidate: String = bytes[start_char..=i].iter().collect();
                    if serde_json::from_str::<serde_json::Value>(&candidate).is_ok() {
                        return Some(candidate);
                    }
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

fn curl_exe() -> String {
    // System32 里有就用绝对路径，避免 PATH 被改动后找不到
    let p = r"C:\Windows\System32\curl.exe";
    if std::path::Path::new(p).is_file() {
        p.to_string()
    } else {
        "curl".to_string()
    }
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

    fn groups() -> Vec<String> {
        vec!["游戏".into(), "开发".into(), "工具".into()]
    }

    fn items() -> Vec<privacy::RedactedItem> {
        vec![
            privacy::RedactedItem {
                name: "Steam".into(),
                ext: ".lnk".into(),
                company: "Valve".into(),
                target_hint: "Steam\\steam.exe".into(),
            },
            privacy::RedactedItem {
                name: "神秘".into(),
                ext: ".lnk".into(),
                company: String::new(),
                target_hint: "X\\x.exe".into(),
            },
        ]
    }

    #[test]
    fn parses_plain_json() {
        let s = r#"{"assignments":[{"file":"Steam.lnk","group":"游戏","reason":"游戏平台"}]}"#;
        let got = parse_assignments(s, &items(), &groups()).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].file_name, "Steam.lnk");
        assert_eq!(got[0].group.as_deref(), Some("游戏"));
    }

    /// 模型经常套代码围栏或加一句客套话。
    #[test]
    fn tolerates_code_fence_and_prose() {
        let s = "好的，结果如下：\n```json\n{\"assignments\":[{\"file\":\"Steam\",\"group\":\"游戏\",\"reason\":\"\"}]}\n```\n希望有帮助。";
        let got = parse_assignments(s, &items(), &groups()).unwrap();
        assert_eq!(got[0].group.as_deref(), Some("游戏"));
    }

    /// 发明出来的分组必须丢弃，否则会在 PecoFence 里找不存在的栅栏。
    #[test]
    fn unknown_group_becomes_none() {
        let s = r#"{"assignments":[{"file":"Steam.lnk","group":"游戏平台","reason":"x"}]}"#;
        let got = parse_assignments(s, &items(), &groups()).unwrap();
        assert_eq!(got[0].group, None);
    }

    #[test]
    fn null_group_is_preserved() {
        let s = r#"{"assignments":[{"file":"神秘.lnk","group":null,"reason":"不认识"}]}"#;
        let got = parse_assignments(s, &items(), &groups()).unwrap();
        assert_eq!(got[0].file_name, "神秘.lnk");
        assert_eq!(got[0].group, None);
    }

    #[test]
    fn unknown_file_is_dropped() {
        let s = r#"{"assignments":[{"file":"不存在.exe","group":"工具","reason":""}]}"#;
        let got = parse_assignments(s, &items(), &groups()).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn missing_assignments_key_is_error() {
        let s = r#"{"result":[]}"#;
        assert!(parse_assignments(s, &items(), &groups()).is_err());
    }

    #[test]
    fn braces_inside_strings_do_not_break_extraction() {
        let s = r#"{"assignments":[{"file":"Steam.lnk","group":"游戏","reason":"含 {花括号} 的说明"}]}"#;
        let got = parse_assignments(s, &items(), &groups()).unwrap();
        assert_eq!(got[0].group.as_deref(), Some("游戏"));
    }

    #[test]
    fn local_mode_detection() {
        let cfg = Config {
            base_url: "http://127.0.0.1:11434/v1".into(),
            api_key: None,
            model: "qwen2.5".into(),
            timeout_secs: 60,
        };
        assert!(cfg.is_local());
        assert!(cfg.describe_mode().contains("本地"));

        let cloud = Config {
            base_url: "https://api.openai.com/v1".into(),
            api_key: Some("k".into()),
            model: "gpt-4o-mini".into(),
            timeout_secs: 60,
        };
        assert!(!cloud.is_local());
        assert!(cloud.describe_mode().contains("云端"));
    }
}
