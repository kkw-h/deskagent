//! 规则编译：把分类结果里**能用 PecoFence 条件表达**的部分沉淀成持久规则。
//!
//! 这里有两套产出，用途不同：
//!
//! - [`compile_ext_rules`]：给 `rule add --ext` 用的简化形式，只按扩展名分组。
//!   判定保守：一个扩展名只对应一个分组时才编译。
//! - [`compile_conds`]：给 `rules.add` 用的完整条件表达式，能把快捷方式也表达出来。
//!
//! ## 四种分类信号在 PecoFence 里的可表达性（对源码核对过，别再凭印象）
//!
//! | 信号 | PecoFence 条件 | 可表达 |
//! |---|---|---|
//! | `ext` | `Cond::Ext` | 是 |
//! | `name_contains` | `Cond::Name { op: Contains }` | 是 |
//! | `target_contains` | `Cond::ShortcutTarget { op: Contains }` | 是 |
//! | `company` | 无对应条件 | **否** |
//!
//! 早先这里写着「没有快捷方式目标路径」，那是错的：`Cond::ShortcutTarget` 一直存在，
//! 而且 `Rule::new` 会把它归进最高优先级的 `Class::Target`。所以快捷方式并非写不成规则，
//! 只是要为每个产品写一条，人工成本高。
//!
//! 真正表达不了的只有 `company`。PE 版本信息里的公司名是 DeskAgent 独有的信号
//! （PecoFence 的 `ItemFacts` 里没有这个字段），这一部分只能靠常驻守护补。
//!
//! ## 为什么 `name_contains` 仍然不自动生成
//!
//! 子串规则的误伤率太高：给「游戏」生成 `Name contains 游戏`，
//! 会把「游戏加加」这种工具也吸进去。所以 `compile_conds` 会产出它，
//! 但由调用方决定要不要用，不默认落地。

use std::collections::BTreeMap;

use deskagent_classify::{Item, RulePack};
use pecofence_core::rules::{Cond, StrOp};

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

/// 一条单条件规则，可以交给 `rules.add`。
///
/// 为什么是「单条件」：规则包里一条规则的多个候选值之间是 **OR**
/// （`company: [A, B]` 表示命中 A 或 B 就算），而 PecoFence 的 `Rule.all_of`
/// 是 **AND**。语义不同，所以一条 YAML 规则要拆成多条 PecoFence 规则。
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRule {
    pub group: String,
    pub cond: Cond,
    /// 人可读的来源描述，用于展示、去重和排查。
    pub source: String,
    /// 原规则包里的优先级，越大越先匹配。
    pub priority: i32,
}

/// 分类用到了、但 PecoFence 条件表达不了的信号。
#[derive(Debug, Clone, PartialEq)]
pub struct Unrepresentable {
    pub group: String,
    /// 目前只有 `"company"`：PE 版本信息里的公司名，`ItemFacts` 没有这个字段。
    pub kind: &'static str,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CompiledConds {
    pub rules: Vec<CompiledRule>,
    pub unrepresentable: Vec<Unrepresentable>,
}

/// 把规则包编译成 PecoFence 原生的 `Cond`。
///
/// 与 [`compile_ext_rules`] 的区别：那个只按扩展名分组，产出给 `rule add --ext`；
/// 这个逐条映射，能把 `target_contains` 也表达出来，产出给 `rules.add`。
///
/// 优先级语义并不完全对齐，这是必须知道的一处差异：
///   - DeskAgent 用数值 priority，大的先匹配；
///   - PecoFence 先按 `Class` 排序（Target > Type > Name > Time > Custom），
///     再按列表顺序。
///
/// 所以这里按 priority 降序输出，但落地后 PecoFence 仍会把 Target 类提到最前。
/// 对当前规则包没有实际影响（各分组之间没有重叠的匹配条件），
/// 但如果以后加了「同一产品在两组之间竞争」的规则，就要重新核对。
pub fn compile_conds(pack: &RulePack) -> CompiledConds {
    let mut out = CompiledConds::default();

    let mut rules: Vec<&deskagent_classify::Rule> = pack.rules.iter().collect();
    // 稳定排序：同优先级保持 yaml 里的先后顺序
    rules.sort_by(|a, b| b.priority.cmp(&a.priority));

    for rule in rules {
        let m = &rule.r#match;

        // 同一组内的多个扩展名是「或」，但 Cond::Ext 本身就接受列表，合并成一条即可。
        let exts: Vec<String> = m
            .ext
            .iter()
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        if !exts.is_empty() {
            out.rules.push(CompiledRule {
                group: rule.group.clone(),
                cond: Cond::Ext(exts.clone()),
                source: format!("扩展名 {}", exts.join(",")),
                priority: rule.priority,
            });
        }

        for t in m.target_contains.iter().filter(|t| !t.is_empty()) {
            out.rules.push(CompiledRule {
                group: rule.group.clone(),
                cond: Cond::ShortcutTarget {
                    op: StrOp::Contains,
                    value: t.to_lowercase(),
                },
                source: format!("目标路径含 {t}"),
                priority: rule.priority,
            });
        }

        for n in m.name_contains.iter().filter(|n| !n.is_empty()) {
            out.rules.push(CompiledRule {
                group: rule.group.clone(),
                cond: Cond::Name {
                    op: StrOp::Contains,
                    value: n.to_lowercase(),
                },
                source: format!("文件名含 {n}"),
                priority: rule.priority,
            });
        }

        let companies: Vec<String> = m
            .company
            .iter()
            .filter(|c| !c.is_empty())
            .cloned()
            .collect();
        if !companies.is_empty() {
            out.unrepresentable.push(Unrepresentable {
                group: rule.group.clone(),
                kind: "company",
                values: companies,
            });
        }
    }

    out
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

    // ---------------------------------------------------- 完整条件编译

    /// 快捷方式目标路径**可以**表达成 PecoFence 条件。早先这里判断错了，
    /// 用测试钉住：Cond::ShortcutTarget 一直存在，且是最高的 Target 类。
    #[test]
    fn target_contains_becomes_shortcut_target_cond() {
        let p = RulePack::from_yaml(
            r#"
rules:
  - group: 游戏
    priority: 130
    match:
      target_contains: ['\steam\', '\epicgames\']
"#,
        )
        .unwrap();
        let c = compile_conds(&p);
        assert_eq!(c.rules.len(), 2, "两个候选值要拆成两条（OR → 多条 AND）");
        assert_eq!(
            c.rules[0].cond,
            Cond::ShortcutTarget {
                op: StrOp::Contains,
                value: r"\steam\".into()
            }
        );
        assert!(c.unrepresentable.is_empty());
    }

    /// 公司名没有对应的 Cond，必须如实报出来，不能悄悄丢掉。
    #[test]
    fn company_is_reported_as_unrepresentable() {
        let p = RulePack::from_yaml(
            r#"
rules:
  - group: 开发
    priority: 120
    match:
      company: ['JetBrains', 'Zed Industries']
      target_contains: ['\jetbrains\']
"#,
        )
        .unwrap();
        let c = compile_conds(&p);
        // target_contains 那一半能表达
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0].group, "开发");
        // company 那一半表达不了，但要被记录
        assert_eq!(c.unrepresentable.len(), 1);
        assert_eq!(c.unrepresentable[0].kind, "company");
        assert_eq!(c.unrepresentable[0].values.len(), 2);
    }

    /// 同一规则的多个扩展名合并成一条 Cond::Ext，避免规则表膨胀。
    #[test]
    fn exts_merge_into_one_cond() {
        let p = RulePack::from_yaml(
            r#"
rules:
  - group: 办公
    priority: 110
    match:
      ext: ['.pdf', '.docx', '.xlsx']
"#,
        )
        .unwrap();
        let c = compile_conds(&p);
        assert_eq!(c.rules.len(), 1);
        assert_eq!(
            c.rules[0].cond,
            Cond::Ext(vec!["pdf".into(), "docx".into(), "xlsx".into()])
        );
    }

    /// 输出按 priority 降序，与分类时的匹配顺序一致。
    #[test]
    fn conds_follow_priority_order() {
        let p = RulePack::from_yaml(
            r#"
rules:
  - group: 办公
    priority: 100
    match:
      ext: ['.pdf']
  - group: 游戏
    priority: 130
    match:
      target_contains: ['\steam\']
"#,
        )
        .unwrap();
        let c = compile_conds(&p);
        let groups: Vec<&str> = c.rules.iter().map(|r| r.group.as_str()).collect();
        assert_eq!(groups, vec!["游戏", "办公"]);
    }
}
