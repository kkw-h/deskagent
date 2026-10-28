//! 规则编译：把分类结果里**能用 PecoFence 条件表达**的部分沉淀成持久规则。
//!
//! 这是整个产品里唯一「一次投入、长期免维护」的部分，所以判定要保守：
//! 只编译扩展名规则，且只在「一个扩展名只对应一个分组」时才编译。
//!
//! 为什么只做扩展名：
//!   - PecoFence 的条件是 `--ext` / `--type` / `--name-contains` / `--glob` 等**文件属性**，
//!     没有公司名，也没有快捷方式目标路径。
//!   - 我们分类时最强的那两个信号（target_contains、company）恰好无法表达成规则。
//!   - `--name-contains` 可以表达一部分，但自动生成的子串规则误伤率太高
//!     （比如给「游戏」生成 `--name-contains 游戏`，会把「游戏加加」这种工具也吸进去），
//!     所以不自动生成，留给用户手写。
//!
//! 推论：扩展名能覆盖的部分可以做到「新文件自动归位」，零维护；
//! 快捷方式那部分必须靠 `deskagent watch` 常驻守护。

use std::collections::BTreeMap;

use deskagent_classify::{Item, RulePack};

/// 快捷方式类扩展名。归属靠语义判断，用扩展名规则只会把所有快捷方式
/// 塞进同一个栅栏，所以不参与编译。
const SEMANTIC_EXTS: [&str; 2] = [".lnk", ".url"];

/// 这个扩展名能否用来表达分类判断。
///
/// 快捷方式不行：所有 `.lnk` 长得一样，规则无法区分 Steam 和 Zed。
/// 对照实验要按「能不能写成规则」把条目分成两拨，所以这个判断要公开。
pub fn is_semantic_ext(ext: &str) -> bool {
    let e = ext.to_lowercase();
    e.is_empty() || SEMANTIC_EXTS.contains(&e.as_str())
}

/// 一条可以直接交给 `rule add` 的规则。
#[derive(Debug, Clone, PartialEq)]
pub struct ExtRule {
    pub group: String,
    /// 同一分组下的扩展名合并成一条规则，减少规则表噪音。
    pub exts: Vec<String>,
}

impl ExtRule {
    /// 规则名带 `deskagent-` 前缀，方便用户识别和删除。
    pub fn name(&self) -> String {
        format!("deskagent-{}", self.group)
    }

    /// `--ext` 的实参形式：逗号分隔、不带点。
    /// 展示时也用这个，避免界面上写着 `.pdf` 而实际传的是 `pdf`。
    pub fn ext_arg(&self) -> String {
        self.exts
            .iter()
            .map(|e| e.trim_start_matches('.'))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// 同一个扩展名出现在多个分组里，说明它本身不携带足够信息，不编译。
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub ext: String,
    /// 分组 → 出现次数，按次数降序。
    pub groups: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Default)]
pub struct Compiled {
    pub rules: Vec<ExtRule>,
    pub conflicts: Vec<Conflict>,
    /// 因为扩展名不可表达（快捷方式、无扩展名）而没有参与编译的条目数。
    pub skipped_semantic: usize,
}

/// 从当前分类结果里推导扩展名规则。
pub fn compile_ext_rules(items: &[Item], pack: &RulePack) -> Compiled {
    // ext → (group → 次数)
    let mut tally: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut skipped_semantic = 0;

    for item in items {
        let ext = item.ext.to_lowercase();
        if is_semantic_ext(&ext) {
            skipped_semantic += 1;
            continue;
        }
        let Some((group, _)) = pack.classify(item) else {
            continue;
        };
        *tally
            .entry(ext)
            .or_default()
            .entry(group)
            .or_insert(0) += 1;
    }

    let mut by_group: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut conflicts = Vec::new();

    for (ext, groups) in tally {
        if groups.len() == 1 {
            let group = groups.keys().next().unwrap().clone();
            by_group.entry(group).or_default().push(ext);
        } else {
            let mut ranked: Vec<(String, usize)> = groups.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            conflicts.push(Conflict { ext, groups: ranked });
        }
    }

    // 按 GROUP_ORDER 排，让规则表顺序与栅栏顺序一致，便于人工核对
    let mut rules: Vec<ExtRule> = by_group
        .into_iter()
        .map(|(group, mut exts)| {
            exts.sort();
            ExtRule { group, exts }
        })
        .collect();
    rules.sort_by_key(|r| {
        let rank = super::GROUP_ORDER
            .iter()
            .position(|g| *g == r.group)
            .unwrap_or(super::GROUP_ORDER.len());
        (rank, r.group.clone())
    });

    Compiled {
        rules,
        conflicts,
        skipped_semantic,
    }
}

/// 从 `rule list` 的返回值里取出已有的规则名。
///
/// 注意 `rule list` 返回的是对象 `{defaultTarget, keepUpdated, unmatched, list}`，
/// 与 `fence list` 的裸数组不同——这里踩过一次。
pub fn existing_rule_names(list: &serde_json::Value) -> Vec<String> {
    list.get("list")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `keepUpdated` 为真时 PecoFence 自己会给新条目套规则，
/// 扩展名那一半就不需要 DeskAgent 常驻了。
pub fn keep_updated(list: &serde_json::Value) -> bool {
    list.get("keepUpdated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, ext: &str, target: &str) -> Item {
        Item {
            name: name.into(),
            file_name: format!("{name}{ext}"),
            path: format!(r"C:\Users\me\Desktop\{name}{ext}"),
            target: target.into(),
            company: String::new(),
            product: String::new(),
            ext: ext.into(),
        }
    }

    fn pack() -> RulePack {
        RulePack::from_yaml(
            r#"
rules:
  - group: 办公
    priority: 110
    match:
      ext: ['.pdf', '.docx']
  - group: 影音
    priority: 110
    match:
      ext: ['.mp4']
  - group: 游戏
    priority: 130
    match:
      target_contains: ['\steam\']
"#,
        )
        .unwrap()
    }

    #[test]
    fn single_group_per_ext_becomes_rule() {
        let items = vec![
            item("合同", ".pdf", ""),
            item("预算", ".pdf", ""),
            item("纪要", ".docx", ""),
        ];
        let c = compile_ext_rules(&items, &pack());
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0].group, "办公");
        // 同一分组的扩展名合并
        assert_eq!(c.rules[0].exts, vec![".docx", ".pdf"]);
        assert!(c.conflicts.is_empty());
    }

    /// 同一个扩展名落在两个分组里，说明它本身不携带分类信息，不能编译。
    #[test]
    fn ext_in_two_groups_is_conflict_not_rule() {
        let p = RulePack::from_yaml(
            r#"
rules:
  - group: 办公
    priority: 110
    match:
      ext: ['.pdf']
  - group: 创作
    priority: 120
    match:
      name_contains: ['设计稿']
"#,
        )
        .unwrap();
        let items = vec![item("合同", ".pdf", ""), item("设计稿A", ".pdf", "")];
        let c = compile_ext_rules(&items, &p);
        assert!(c.rules.is_empty(), "有冲突时不应产出规则");
        assert_eq!(c.conflicts.len(), 1);
        assert_eq!(c.conflicts[0].ext, ".pdf");
        assert_eq!(c.conflicts[0].groups.len(), 2);
    }

    /// .lnk 是语义判断，用扩展名规则会把所有快捷方式塞进一个栅栏。
    #[test]
    fn shortcuts_never_become_ext_rules() {
        let items = vec![
            item("Steam", ".lnk", r"C:\Program Files (x86)\Steam\steam.exe"),
            item("Chrome", ".lnk", r"C:\Program Files\Google\Chrome\chrome.exe"),
        ];
        let c = compile_ext_rules(&items, &pack());
        assert!(c.rules.is_empty());
        assert_eq!(c.skipped_semantic, 2);
    }

    #[test]
    fn unclassified_items_are_ignored() {
        let items = vec![item("神秘文件", ".xyz", "")];
        let c = compile_ext_rules(&items, &pack());
        assert!(c.rules.is_empty());
        assert!(c.conflicts.is_empty());
    }

    #[test]
    fn rule_name_is_prefixed_for_findability() {
        let r = ExtRule {
            group: "办公".into(),
            exts: vec![".pdf".into()],
        };
        assert_eq!(r.name(), "deskagent-办公");
    }

    /// 展示和实参必须一致：PecoFence 的 --ext 不带点。
    #[test]
    fn ext_arg_has_no_dots() {
        let r = ExtRule {
            group: "办公".into(),
            exts: vec![".docx".into(), ".pdf".into(), ".xlsx".into()],
        };
        assert_eq!(r.ext_arg(), "docx,pdf,xlsx");
    }

    #[test]
    fn rules_follow_group_order() {
        let items = vec![item("视频", ".mp4", ""), item("合同", ".pdf", "")];
        let c = compile_ext_rules(&items, &pack());
        // GROUP_ORDER 里 影音 在 办公 之前
        let titles: Vec<&str> = c.rules.iter().map(|r| r.group.as_str()).collect();
        assert_eq!(titles, vec!["影音", "办公"]);
    }

    // ---------------------------------------------------- rule list 解析

    /// rule list 返回对象，fence list 返回裸数组。这里混过一次，用测试钉住。
    #[test]
    fn rule_names_come_from_list_field() {
        let v = serde_json::json!({
            "defaultTarget": "inbox",
            "keepUpdated": true,
            "unmatched": "defaultTarget",
            "list": [
                {"index": 0, "id": "r1", "name": "deskagent-办公", "enabled": true},
                {"index": 1, "id": "r2", "name": "PDFs", "enabled": false}
            ]
        });
        assert_eq!(existing_rule_names(&v), vec!["deskagent-办公", "PDFs"]);
        assert!(keep_updated(&v));
    }

    #[test]
    fn bare_array_is_not_mistaken_for_rule_list() {
        // 如果误把 fence list 的形状用在这里，应该得到空而不是 panic
        let v = serde_json::json!([{"title": "游戏"}]);
        assert!(existing_rule_names(&v).is_empty());
        assert!(!keep_updated(&v));
    }
}
