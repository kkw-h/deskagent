//! 用户纠正的记忆。
//!
//! 语义整理靠规则包判断，但包认不出来、或者认错了的，用户会自己把图标拖到正确的栅栏里。
//! 那不是噪音，是标注：下一次同一个软件出现时应当照样处理。这里只存最小的可泛化信号——
//! 公司名优先（`Valve` 能覆盖一整族游戏），其次才是文件名；地址、路径这类会变的都不存。
//!
//! 记录放在 `config.json` 的 `learnings` 里，跟着配置一起备份、一起导出。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 一次纠正的记忆表。键都按小写存，比较时也转小写。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Learnings {
    /// 公司名 → 栅栏标题。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_company: BTreeMap<String, String>,
    /// 文件名（含扩展名）→ 栅栏标题。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_name: BTreeMap<String, String>,
}

impl Learnings {
    pub fn is_empty(&self) -> bool {
        self.by_company.is_empty() && self.by_name.is_empty()
    }

    /// 记下的条数。
    pub fn len(&self) -> usize {
        self.by_company.len() + self.by_name.len()
    }

    /// 记下一次纠正。公司名可用时只记公司名——它比文件名更能泛化，也不会让记忆表按文件增长。
    ///
    /// 返回 `true` 表示记下了（组名非空）。组名没有变化时不重复写，避免无谓的保存。
    pub fn remember(&mut self, company: &str, file_name: &str, group: &str) -> bool {
        let group = group.trim();
        if group.is_empty() {
            return false;
        }
        let company = company.trim().to_lowercase();
        let key = if !company.is_empty() {
            &mut self.by_company
        } else {
            let name = file_name.trim().to_lowercase();
            if name.is_empty() {
                return false;
            }
            return changed(&mut self.by_name, name, group);
        };
        changed(key, company, group)
    }

    /// 这个条目该去哪儿：公司名优先，其次文件名。
    pub fn lookup(&self, company: &str, file_name: &str) -> Option<&str> {
        let company = company.trim().to_lowercase();
        if !company.is_empty()
            && let Some(group) = self.by_company.get(&company)
        {
            return Some(group);
        }
        let name = file_name.trim().to_lowercase();
        if name.is_empty() {
            return None;
        }
        self.by_name.get(&name).map(String::as_str)
    }

    /// 栅栏被删掉时忘掉指向它的记忆，免得下次又冒出一个同名栅栏。返回清掉的条数。
    pub fn forget_group(&mut self, group: &str) -> usize {
        let before = self.len();
        self.by_company.retain(|_, g| g != group);
        self.by_name.retain(|_, g| g != group);
        before - self.len()
    }

    /// 栅栏改名：记忆跟着一起改，否则旧标题会被当成一个新分组。返回改动的条数。
    pub fn rename_group(&mut self, old: &str, new: &str) -> usize {
        let mut changed = 0;
        for group in self
            .by_company
            .values_mut()
            .chain(self.by_name.values_mut())
        {
            if group == old {
                *group = new.to_string();
                changed += 1;
            }
        }
        changed
    }

    pub fn clear(&mut self) {
        self.by_company.clear();
        self.by_name.clear();
    }
}

fn changed(map: &mut BTreeMap<String, String>, key: String, group: &str) -> bool {
    if map.get(&key).is_some_and(|g| g == group) {
        return false;
    }
    map.insert(key, group.to_string());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn company_remembers_the_whole_vendor_not_one_file() {
        let mut learn = Learnings::default();
        assert!(learn.remember("Valve", "Steam.lnk", "游戏"));
        // 同厂的新条目：直接命中，不用再看文件名。
        assert_eq!(learn.lookup("Valve", "Half-Life.lnk"), Some("游戏"));
        assert_eq!(learn.lookup(" valve ", "anything.lnk"), Some("游戏"));
        // 没有公司名时才退回文件名。
        assert_eq!(learn.lookup("", "Steam.lnk"), None);
        assert_eq!(learn.len(), 1);
    }

    #[test]
    fn unknown_items_have_no_answer() {
        let learn = Learnings::default();
        assert_eq!(learn.lookup("Valve", "Steam.lnk"), None);
        assert!(learn.is_empty());
    }

    #[test]
    fn without_a_company_the_file_name_is_the_key() {
        let mut learn = Learnings::default();
        assert!(learn.remember("", "MysteryTool.exe", "工具"));
        assert_eq!(learn.lookup("", "mysterytool.exe"), Some("工具"));
        assert_eq!(learn.lookup("Someone", "MysteryTool.exe"), Some("工具"));
    }

    #[test]
    fn remembering_the_same_answer_again_is_not_a_change() {
        let mut learn = Learnings::default();
        assert!(learn.remember("Valve", "Steam.lnk", "游戏"));
        assert!(!learn.remember("Valve", "Steam.lnk", "游戏"));
        assert!(learn.remember("Valve", "Steam.lnk", "开发"));
        assert_eq!(learn.lookup("Valve", "Steam.lnk"), Some("开发"));
    }

    #[test]
    fn deleting_a_fence_forgets_its_groups() {
        let mut learn = Learnings::default();
        learn.remember("Valve", "Steam.lnk", "游戏");
        learn.remember("", "mystery.exe", "游戏");
        learn.remember("Zed", "Zed.exe", "开发");
        assert_eq!(learn.forget_group("游戏"), 2);
        assert_eq!(learn.lookup("Valve", "Steam.lnk"), None);
        assert_eq!(learn.lookup("Zed", "Zed.exe"), Some("开发"));
    }

    #[test]
    fn renaming_a_fence_moves_its_memories() {
        let mut learn = Learnings::default();
        learn.remember("Valve", "Steam.lnk", "游戏");
        learn.remember("", "mystery.exe", "游戏");
        learn.remember("Zed", "Zed.exe", "开发");
        assert_eq!(learn.rename_group("游戏", "Games"), 2);
        assert_eq!(learn.lookup("Valve", "Steam.lnk"), Some("Games"));
        assert_eq!(learn.lookup("Zed", "Zed.exe"), Some("开发"));
    }
}
