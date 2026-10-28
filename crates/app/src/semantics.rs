//! 语义分类：DeskAgent 的规则包和采集逻辑现在跑在应用本体里。
//!
//! 并入之前，这一步只存在于独立的 `deskagent` 可执行文件里：它重新扫描桌面、
//! 自己解析 `.lnk`，再通过 IPC 让应用建栅栏。现在应用自己就能回答
//! 「这个图标是什么软件、该放进哪个栅栏」，不再需要外部进程。
//!
//! 这一层只做判断不做 IO（除了读 PE 版本信息），所以能纯函数测试；
//! 建栅栏、移动条目在 `app::semantics` 里做。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use deskagent_classify::{Item, RulePack};
use pecofence_platform::shell::{self, DesktopEntry, EntryOrigin};

/// 与 `deskagent` 二进制共用同一份规则包，编进二进制，运行期不依赖文件路径。
const BUILTIN_RULES: &str = include_str!("../../../rules/builtin.yaml");

/// 内置规则包（首次使用时解析，之后复用）。
pub fn pack() -> &'static RulePack {
    static PACK: OnceLock<RulePack> = OnceLock::new();
    PACK.get_or_init(|| {
        RulePack::from_yaml(BUILTIN_RULES).unwrap_or(RulePack { rules: Vec::new() })
    })
}

/// 一个准备归入某个分组的条目。
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// 显示名（不含扩展名）。
    pub name: String,
    /// 完整路径，用于在状态里找回条目。
    pub path: String,
    /// 命中依据：规则里的短语，或 `uncovered` / `belowThreshold`。
    pub detail: String,
}

/// 一个分组，标题就是栅栏标题（「游戏」「开发」…）。
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub title: String,
    pub items: Vec<Planned>,
}

/// 一次语义分类的完整结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// 达到条目阈值、值得建栅栏的分组，已按 [`deskagent_compile::GROUP_ORDER`] 排序。
    pub groups: Vec<Group>,
    /// 规则没认出来、或分组太小而留在桌面的条目。
    pub unclassified: Vec<Planned>,
    /// 参与分类的条目总数（不含命名空间项）。
    pub total: usize,
}

impl Plan {
    /// 会被移进栅栏的条目数。
    pub fn moved(&self) -> usize {
        self.groups.iter().map(|g| g.items.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// 把一个桌面条目变成分类器认识的记录。
///
/// 命名空间项（回收站、此电脑）没有文件可读，返回 `None`。
pub fn item_for(entry: &DesktopEntry) -> Option<Item> {
    if entry.origin == EntryOrigin::Namespace {
        return None;
    }
    let path = entry.path.to_string_lossy().into_owned();
    let lower = entry.file_name.to_lowercase();
    // 快捷方式看目标；普通文件的目标就是自己。
    let target = if lower.ends_with(".lnk") {
        shell::shortcut_target(&entry.path).unwrap_or_else(|| path.clone())
    } else if lower.ends_with(".url") {
        shell::url_shortcut_target(&entry.path).unwrap_or_default()
    } else {
        path.clone()
    };
    // 公司名/产品名只在目标是 .exe 时读，与 `AppState::facts_at` 的取法一致。
    let version = target
        .to_lowercase()
        .ends_with(".exe")
        .then(|| deskagent_scan::read_version(&target))
        .flatten();
    let name = entry
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| entry.file_name.clone());
    Some(Item {
        name,
        file_name: entry.file_name.clone(),
        path,
        target,
        company: version
            .as_ref()
            .map(|v| v.company.clone())
            .unwrap_or_default(),
        product: version
            .as_ref()
            .map(|v| v.product.clone())
            .unwrap_or_default(),
        ext: pecofence_core::rules::ext_of(&entry.file_name),
    })
}

/// 分类 `entries`，按 `min_items` 决定多小的分组不值得建栅栏。
pub fn plan(entries: &[DesktopEntry], min_items: usize) -> Plan {
    let pack = pack();
    let min_items = min_items.max(1);
    let mut grouped: BTreeMap<String, Vec<Planned>> = BTreeMap::new();
    let mut unclassified = Vec::new();
    let mut total = 0;

    for entry in entries {
        let Some(item) = item_for(entry) else {
            continue;
        };
        total += 1;
        let planned = Planned {
            name: item.name.clone(),
            path: item.path.clone(),
            detail: String::new(),
        };
        match pack.classify(&item) {
            Some((group, reason)) => {
                grouped.entry(group).or_default().push(Planned {
                    detail: reason,
                    ..planned
                });
            }
            None => unclassified.push(Planned {
                detail: "uncovered".into(),
                ..planned
            }),
        }
    }

    // 先按预设顺序（游戏 → 浏览器 → 开发 → …），未知分组按名称排在后面，
    // 保证多次运行的栅栏顺序稳定。
    let mut ordered: Vec<(usize, String, Vec<Planned>)> = grouped
        .into_iter()
        .map(|(title, mut items)| {
            items.sort_by(|a, b| a.path.cmp(&b.path));
            let rank = deskagent_compile::GROUP_ORDER
                .iter()
                .position(|g| *g == title)
                .unwrap_or(deskagent_compile::GROUP_ORDER.len());
            (rank, title, items)
        })
        .collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut groups = Vec::new();
    for (_rank, title, items) in ordered {
        if items.len() < min_items {
            unclassified.extend(items.into_iter().map(|item| Planned {
                detail: "belowThreshold".into(),
                ..item
            }));
            continue;
        }
        groups.push(Group { title, items });
    }
    Plan {
        groups,
        unclassified,
        total,
    }
}

/// 归类时的默认阈值：一个图标的栅栏只是把桌面变复杂。
pub const DEFAULT_MIN_ITEMS: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str, origin: EntryOrigin, is_folder: bool) -> DesktopEntry {
        DesktopEntry {
            path: PathBuf::from(format!(r"C:\Users\me\Desktop\{name}")),
            file_name: name.into(),
            origin,
            is_folder,
            attributes: 0,
            mtime: 0,
            size: 0,
            created: 0,
        }
    }

    fn file(name: &str) -> DesktopEntry {
        entry(name, EntryOrigin::UserDesktop, false)
    }

    #[test]
    fn builtin_pack_parses_and_has_rules() {
        assert!(pack().rules.len() > 20, "内置规则包应该有成规模的规则");
    }

    #[test]
    fn documents_and_media_land_in_their_groups() {
        let entries = vec![
            file("renewal.pdf"),
            file("invoice.pdf"),
            file("clip.mp4"),
            file("demo.mkv"),
        ];
        let plan = plan(&entries, DEFAULT_MIN_ITEMS);
        let titles: Vec<&str> = plan.groups.iter().map(|g| g.title.as_str()).collect();
        // GROUP_ORDER 里 影音 在 办公 之前
        assert_eq!(titles, vec!["影音", "办公"]);
        assert_eq!(plan.moved(), 4);
        assert_eq!(plan.total, 4);
        assert!(plan.unclassified.is_empty());
    }

    #[test]
    fn groups_below_the_threshold_stay_on_the_desktop() {
        let entries = vec![file("renewal.pdf"), file("clip.mp4")];
        let plan = plan(&entries, 2);
        assert!(plan.groups.is_empty());
        assert_eq!(plan.unclassified.len(), 2);
        assert!(
            plan.unclassified
                .iter()
                .all(|p| p.detail == "belowThreshold")
        );
    }

    #[test]
    fn unknown_extensions_are_reported_not_guessed() {
        let entries = vec![file("mystery.qqq"), file("another.qqq")];
        let plan = plan(&entries, DEFAULT_MIN_ITEMS);
        assert!(plan.groups.is_empty());
        assert!(plan.unclassified.iter().all(|p| p.detail == "uncovered"));
    }

    #[test]
    fn namespace_entries_are_skipped_entirely() {
        let entries = vec![
            entry(
                "::{645FF040-5081-101B-9F08-00AA002F954E}",
                EntryOrigin::Namespace,
                false,
            ),
            file("a.pdf"),
            file("b.pdf"),
        ];
        let plan = plan(&entries, 1);
        assert_eq!(plan.total, 2);
        assert_eq!(plan.groups.len(), 1);
    }

    #[test]
    fn folders_are_not_classified_by_extension() {
        let entries = vec![entry("Projects", EntryOrigin::UserDesktop, true)];
        let plan = plan(&entries, 1);
        assert!(plan.groups.is_empty());
        assert_eq!(plan.unclassified.len(), 1);
    }
}
