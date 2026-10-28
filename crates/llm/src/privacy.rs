//! 隐私：上传给模型的内容必须先去标识化。
//!
//! 规则包判不出来的条目才走模型，而走模型时最危险的字段是路径——
//! `C:\Users\张三\Desktop\客户名单.xlsx` 里的用户名和文件名本身就是隐私。
//!
//! 做法：路径只保留「产品目录名 + 可执行文件名」这两段，
//! 去掉盘符、用户名、桌面路径。这样 `C:\Program Files (x86)\Steam\steam.exe`
//! 变成 `Steam\steam.exe`，模型仍能判断，但拿不到任何用户信息。

/// 系统目录前缀，去掉它们之后剩下的才是产品信息。
const SYSTEM_PREFIXES: [&str; 6] = [
    r"\program files (x86)\",
    r"\program files\",
    r"\appdata\local\programs\",
    r"\appdata\local\",
    r"\appdata\roaming\",
    r"\programdata\",
];

/// 把绝对路径压成「最多两段」的相对片段。
pub fn sanitize_path(path: &str) -> String {
    let lower = path.to_lowercase();

    // 命中系统前缀时，从前缀之后开始算，前面全是机器相关信息
    let mut rest = path;
    for p in SYSTEM_PREFIXES {
        if let Some(idx) = lower.find(p) {
            rest = &path[idx + p.len()..];
            break;
        }
    }

    // 没命中前缀（比如 D:\Games\...、C:\Users\me\Desktop\...）就只取末尾两段
    let parts: Vec<&str> = rest.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    let take = parts.len().min(2);
    parts[parts.len() - take..].join("\\")
}

/// URL 只保留主机名：`https://mail.qq.com/cgi-bin/x?sid=abc` → `mail.qq.com`。
pub fn sanitize_url(url: &str) -> String {
    let rest = url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(url);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

/// 给模型看的条目视图。字段全是分类必需、且不泄露身份的信息。
#[derive(Debug, Clone, PartialEq)]
pub struct RedactedItem {
    pub name: String,
    pub ext: String,
    pub company: String,
    /// 去标识化后的目标路径片段。
    pub target_hint: String,
}

impl RedactedItem {
    pub fn from_item(item: &deskagent_classify::Item) -> Self {
        let target_hint = if item.target.starts_with("http://") || item.target.starts_with("https://")
        {
            sanitize_url(&item.target)
        } else if item.target.is_empty() {
            String::new()
        } else {
            sanitize_path(&item.target)
        };
        Self {
            name: item.name.clone(),
            ext: item.ext.clone(),
            company: item.company.clone(),
            target_hint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_program_files_prefix() {
        assert_eq!(
            sanitize_path(r"C:\Program Files (x86)\Steam\steam.exe"),
            "Steam\\steam.exe"
        );
        assert_eq!(
            sanitize_path(r"C:\Program Files\Zed\Zed.exe"),
            "Zed\\Zed.exe"
        );
    }

    #[test]
    fn strips_appdata_and_keeps_product_segment() {
        assert_eq!(
            sanitize_path(r"C:\Users\kkw\AppData\Local\Programs\Zed\Zed.exe"),
            "Zed\\Zed.exe"
        );
        assert_eq!(
            sanitize_path(r"C:\Users\kkw\AppData\Roaming\Telegram Desktop\Telegram.exe"),
            "Telegram Desktop\\Telegram.exe"
        );
    }

    /// 桌面上的文件路径带用户名，绝不能原样上传。
    #[test]
    fn user_name_never_survives() {
        let out = sanitize_path(r"C:\Users\张三\Desktop\客户名单.xlsx");
        assert!(!out.contains("张三"), "用户名泄露: {out}");
        assert!(!out.to_lowercase().contains("users"), "用户目录泄露: {out}");
        assert_eq!(out, "Desktop\\客户名单.xlsx");
    }

    #[test]
    fn long_install_paths_keep_last_two_segments() {
        assert_eq!(
            sanitize_path(r"D:\Program Files (x86)\WeGame\wegame.exe"),
            "WeGame\\wegame.exe"
        );
    }

    #[test]
    fn url_keeps_only_host() {
        assert_eq!(
            sanitize_url("https://mail.qq.com/cgi-bin/x?sid=secret"),
            "mail.qq.com"
        );
        assert_eq!(sanitize_url("http://support.steampowered.com/"), "support.steampowered.com");
    }

    #[test]
    fn empty_target_stays_empty() {
        assert_eq!(sanitize_path(""), "");
    }

    #[test]
    fn redacted_item_picks_the_right_sanitizer() {
        let item = deskagent_classify::Item {
            name: "Steam".into(),
            file_name: "Steam.lnk".into(),
            path: r"C:\Users\张三\Desktop\Steam.lnk".into(),
            target: r"C:\Program Files (x86)\Steam\steam.exe".into(),
            company: "Valve Corporation".into(),
            product: String::new(),
            ext: ".lnk".into(),
        };
        let r = RedactedItem::from_item(&item);
        assert_eq!(r.target_hint, "Steam\\steam.exe");
        assert!(!format!("{r:?}").contains("张三"));
    }
}
