//! 分类：把采集到的条目映射到分组。

use serde::{Deserialize, Serialize};

/// 桌面上的一个条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// 显示名（不含扩展名）。
    pub name: String,
    /// 原始文件名。
    pub file_name: String,
    /// 完整路径。
    pub path: String,
    /// 快捷方式目标；非快捷方式时与 path 相同。
    pub target: String,
    /// PE 版本信息中的公司名。
    pub company: String,
    /// PE 版本信息中的产品名。
    pub product: String,
    /// 扩展名，小写，含点。
    pub ext: String,
}

/// 分类结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub item: Item,
    pub group: String,
    /// 0.0–1.0，来自规则置信度或历史命中率。
    pub confidence: f32,
    /// 命中依据，便于用户理解为什么这么分。
    pub reason: String,
}

/// 一条分类规则。
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub group: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub r#match: Match,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Match {
    #[serde(default)]
    pub company: Vec<String>,
    #[serde(default)]
    pub target_contains: Vec<String>,
    #[serde(default)]
    pub name_contains: Vec<String>,
    #[serde(default)]
    pub ext: Vec<String>,
}

/// 规则包。
#[derive(Debug, Clone, Deserialize)]
pub struct RulePack {
    pub rules: Vec<Rule>,
}

impl RulePack {
    pub fn from_yaml(s: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(s)
    }

    /// 按优先级从高到低匹配，返回第一个命中。
    pub fn classify(&self, item: &Item) -> Option<(String, String)> {
        self.classify_with_rule(item)
            .map(|(_, group, reason)| (group, reason))
    }

    /// 同 [`classify`](Self::classify)，额外返回命中规则的下标。
    ///
    /// 对照实验要按「用户手写了多少条规则」计成本，所以必须知道命中的是哪一条，
    /// 而不是只看分到了哪个组。
    pub fn classify_with_rule(&self, item: &Item) -> Option<(usize, String, String)> {
        let mut order: Vec<usize> = (0..self.rules.len()).collect();
        // 稳定排序：同优先级保持 yaml 里的先后顺序
        order.sort_by(|a, b| self.rules[*b].priority.cmp(&self.rules[*a].priority));

        for idx in order {
            let rule = &self.rules[idx];
            if let Some(reason) = rule.matches(item) {
                return Some((idx, rule.group.clone(), reason));
            }
        }
        None
    }
}

impl Rule {
    fn matches(&self, item: &Item) -> Option<String> {
        let m = &self.r#match;
        let company = item.company.to_lowercase();
        let target = item.target.to_lowercase();
        let name = item.name.to_lowercase();
        let ext = item.ext.to_lowercase();

        for c in &m.company {
            if !c.is_empty() && company.contains(&c.to_lowercase()) {
                return Some(format!("公司名匹配 {c}"));
            }
        }
        for t in &m.target_contains {
            if !t.is_empty() && target.contains(&t.to_lowercase()) {
                return Some(format!("目标路径含 {t}"));
            }
        }
        for n in &m.name_contains {
            if !n.is_empty() && name.contains(&n.to_lowercase()) {
                return Some(format!("文件名含 {n}"));
            }
        }
        for e in &m.ext {
            if !e.is_empty() && ext == e.to_lowercase() {
                return Some(format!("扩展名 {e}"));
            }
        }
        None
    }
}
