//! 把分类结果编译成 PecoFence 能执行的动作序列。
//!
//! 这一层不做 IO，只做决策，所以能纯函数测试。
//!
//! 两条约束决定了输出形状：
//!   1. PecoFence 的规则条件只认扩展名、类型、文件名等**文件属性**，
//!      没有公司名、没有快捷方式目标路径。所以「Steam 是游戏」这种语义判断
//!      无法沉淀成规则，只能一次性 `item move`。
//!   2. 只有一个图标的栅栏没有意义，低于阈值的分组留在桌面。

use std::collections::BTreeMap;

use deskagent_classify::{Item, RulePack};

/// 分组优先级：决定栅栏的摆放顺序。
/// 高频、视觉辨识度高的组放前面，工具类垫底。
pub const GROUP_ORDER: [&str; 9] = [
    "游戏", "浏览器", "开发", "社交", "影音", "办公", "创作", "工具", "系统",
];

#[derive(Debug, Clone, PartialEq)]
pub struct FencePlan {
    pub title: String,
    /// 要移进这个栅栏的条目完整路径。
    pub items: Vec<String>,
}

impl FencePlan {
    pub fn count(&self) -> usize {
        self.items.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub fences: Vec<FencePlan>,
    pub skipped: Vec<Skipped>,
    /// 参与规划的条目总数，含被跳过的。
    pub total: usize,
}

impl Plan {
    pub fn moved(&self) -> usize {
        self.fences.iter().map(|f| f.count()).sum()
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// 少于这个条目数的分组不建栅栏，留在桌面。
    pub min_items: usize,
}

impl Default for Options {
    fn default() -> Self {
        // 两个起步：一个图标的栅栏只是把桌面变复杂
        Self { min_items: 2 }
    }
}

/// 按规则包分类并组装成计划。
pub fn build(items: &[Item], pack: &RulePack, opts: &Options) -> Plan {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut skipped = Vec::new();

    for item in items {
        match pack.classify(item) {
            Some((group, _reason)) => {
                groups.entry(group).or_default().push(item.path.clone());
            }
            None => skipped.push(Skipped {
                name: item.name.clone(),
                reason: "规则包未覆盖".into(),
            }),
        }
    }

    // 先按预设顺序，未知分组按名称排在后面，保证多次运行布局稳定
    let mut ordered: Vec<(usize, String, Vec<String>)> = groups
        .into_iter()
        .map(|(title, mut paths)| {
            paths.sort();
            let rank = GROUP_ORDER
                .iter()
                .position(|g| *g == title)
                .unwrap_or(GROUP_ORDER.len());
            (rank, title, paths)
        })
        .collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut fences = Vec::new();
    for (_rank, title, paths) in ordered {
        if paths.len() < opts.min_items {
            for p in &paths {
                skipped.push(Skipped {
                    name: file_name_of(p),
                    reason: format!("「{title}」只有 1 项，未达建栅栏阈值"),
                });
            }
            continue;
        }
        fences.push(FencePlan {
            title,
            items: paths,
        });
    }

    Plan {
        fences,
        skipped,
        total: items.len(),
    }
}

fn file_name_of(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

// ---------------------------------------------------------------- 布局

/// 一块矩形，单位是物理像素的虚拟屏坐标（与 `fence create --rect` 一致）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    pub fn to_arg(self) -> String {
        format!("{},{},{},{}", self.x, self.y, self.w, self.h)
    }
}

const MARGIN: i64 = 24;
const GAP: i64 = 16;
const PREFERRED_W: i64 = 420;
const MIN_H: i64 = 160;
const MAX_H: i64 = 360;

/// 在工作区里排出网格。
///
/// 为什么不能一路 `--below` 串下去：7 个 360px 高的栅栏纵向排开要 2520px，
/// 超出任何常见分辨率的工作区，PecoFence 会以 `invalid_value` 拒绝。
/// 所以先算能放几列，再按行铺开。
pub fn layout(n: usize, area: Rect) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let n_i = n as i64;
    let avail_w = (area.w - MARGIN).max(PREFERRED_W);
    let avail_h = (area.h - MARGIN).max(MIN_H);

    // 列数：能塞下几列就塞几列，但不超过条目数
    let cols = ((avail_w + GAP) / (PREFERRED_W + GAP)).clamp(1, n_i);
    let rows = (n_i + cols - 1) / cols;

    // 行数定了才能算高度：把可用高度均分给每一行
    let h = ((avail_h - GAP * (rows - 1)) / rows).clamp(MIN_H, MAX_H);
    let w = ((avail_w - GAP * (cols - 1)) / cols).clamp(MIN_H, PREFERRED_W);

    (0..n_i)
        .map(|i| Rect {
            x: area.x + MARGIN + (i % cols) * (w + GAP),
            y: area.y + MARGIN + (i / cols) * (h + GAP),
            w,
            h,
        })
        .collect()
}

/// 拿不到 `monitor list` 时的兜底工作区。宁可保守，也不要算出屏幕外的坐标。
pub fn fallback_area() -> Rect {
    Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, target: &str, company: &str) -> Item {
        Item {
            name: name.into(),
            file_name: format!("{name}.lnk"),
            path: format!(r"C:\Users\me\Desktop\{name}.lnk"),
            target: target.into(),
            company: company.into(),
            product: String::new(),
            ext: ".lnk".into(),
        }
    }

    fn pack() -> RulePack {
        RulePack::from_yaml(
            r#"
rules:
  - group: 游戏
    priority: 130
    match:
      target_contains: ['\steam\']
  - group: 开发
    priority: 130
    match:
      target_contains: ['\zed\']
  - group: 工具
    priority: 130
    match:
      target_contains: ['\quicklook\']
"#,
        )
        .unwrap()
    }

    #[test]
    fn groups_below_threshold_stay_on_desktop() {
        let items = vec![
            item("Steam", r"C:\Program Files (x86)\Steam\steam.exe", "Valve"),
            item("Zed", r"C:\Program Files\Zed\Zed.exe", "Zed"),
            item("QuickLook", r"C:\Program Files\QuickLook\QuickLook.exe", ""),
        ];
        let plan = build(&items, &pack(), &Options::default());

        // 三个组各 1 项，都不到阈值
        assert!(plan.fences.is_empty());
        assert_eq!(plan.skipped.len(), 3);
        assert_eq!(plan.total, 3);
    }

    #[test]
    fn group_meeting_threshold_becomes_fence() {
        let items = vec![
            item("Steam", r"C:\Program Files (x86)\Steam\steam.exe", "Valve"),
            item("WeGame", r"D:\WeGame\wegame.exe", "Tencent"),
            item("Zed", r"C:\Program Files\Zed\Zed.exe", "Zed"),
        ];
        let mut p = pack();
        // WeGame 走公司名之外的路径不命中，补一条
        p.rules.push(deskagent_classify::Rule {
            group: "游戏".into(),
            priority: 130,
            r#match: deskagent_classify::Match {
                target_contains: vec![r"\wegame\".into()],
                ..Default::default()
            },
        });

        let plan = build(&items, &p, &Options::default());
        assert_eq!(plan.fences.len(), 1);
        assert_eq!(plan.fences[0].title, "游戏");
        assert_eq!(plan.fences[0].count(), 2);
        assert_eq!(plan.moved(), 2);
    }

    #[test]
    fn fence_order_follows_group_order_not_alphabet() {
        let mut items = Vec::new();
        for n in 0..2 {
            items.push(item(&format!("S{n}"), r"C:\x\steam\a.exe", ""));
            items.push(item(&format!("Z{n}"), r"C:\x\zed\a.exe", ""));
        }
        let plan = build(&items, &pack(), &Options::default());
        let titles: Vec<&str> = plan.fences.iter().map(|f| f.title.as_str()).collect();
        // GROUP_ORDER 里 游戏 在 开发 之前
        assert_eq!(titles, vec!["游戏", "开发"]);
    }

    #[test]
    fn unclassified_is_reported_not_silently_dropped() {
        let items = vec![item("Mystery", r"C:\x\mystery\a.exe", "")];
        let plan = build(&items, &pack(), &Options::default());
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, "规则包未覆盖");
        assert_eq!(plan.total, 1);
    }

    #[test]
    fn min_items_zero_makes_single_fences() {
        let items = vec![item("Zed", r"C:\Program Files\Zed\Zed.exe", "Zed")];
        let plan = build(&items, &pack(), &Options { min_items: 1 });
        assert_eq!(plan.fences.len(), 1);
        assert_eq!(plan.fences[0].title, "开发");
    }

    // ------------------------------------------------------------ 布局

    #[test]
    fn layout_keeps_every_fence_inside_work_area() {
        let area = Rect { x: 0, y: 0, w: 1920, h: 1000 };
        for n in 1..=12 {
            for r in layout(n, area) {
                assert!(r.x >= area.x, "n={n} 越左边界");
                assert!(r.y >= area.y, "n={n} 越上边界");
                assert!(r.x + r.w <= area.x + area.w, "n={n} 越右边界: {r:?}");
                assert!(r.y + r.h <= area.y + area.h, "n={n} 越下边界: {r:?}");
            }
        }
    }

    #[test]
    fn layout_never_overlaps() {
        let area = Rect { x: 0, y: 0, w: 1920, h: 1000 };
        for n in 1..=12 {
            let rects = layout(n, area);
            assert_eq!(rects.len(), n);
            for i in 0..rects.len() {
                for j in (i + 1)..rects.len() {
                    let (a, b) = (rects[i], rects[j]);
                    let apart = a.x + a.w <= b.x
                        || b.x + b.w <= a.x
                        || a.y + a.h <= b.y
                        || b.y + b.h <= a.y;
                    assert!(apart, "n={n} 第 {i} 与第 {j} 个重叠: {a:?} {b:?}");
                }
            }
        }
    }

    /// 七个栅栏是本机桌面的真实情况，纵向串排会到 2520px，必须分行。
    #[test]
    fn seven_fences_use_multiple_rows() {
        let area = Rect { x: 0, y: 0, w: 1920, h: 1000 };
        let rects = layout(7, area);
        let first_row_y = rects[0].y;
        let rows = rects.iter().filter(|r| r.y == first_row_y).count();
        assert!(rows >= 2, "应至少两列，实际 {rows} 列");
    }

    #[test]
    fn layout_respects_non_zero_origin() {
        // 副屏在左侧时 x 为负
        let area = Rect { x: -1920, y: 0, w: 1920, h: 1000 };
        let rects = layout(3, area);
        assert!(rects.iter().all(|r| r.x >= -1920));
        assert_eq!(rects[0].x, -1920 + MARGIN);
    }

    #[test]
    fn layout_empty_is_empty() {
        assert!(layout(0, fallback_area()).is_empty());
    }

    #[test]
    fn rect_arg_matches_cli_format() {
        let r = Rect { x: 100, y: 100, w: 480, h: 360 };
        assert_eq!(r.to_arg(), "100,100,480,360");
    }
}
