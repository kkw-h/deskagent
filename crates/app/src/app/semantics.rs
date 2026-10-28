//! 语义整理的落地层：预览、应用，以及新桌面条目的自动归类。
//!
//! 判断在 [`crate::semantics`]（纯函数）；这里只碰应用状态：建栅栏、移条目、存快照。
//! 与 `deskagent` 二进制以前做的事一样，区别是现在不需要外部进程和 IPC 往返。

use super::*;
use crate::semantics as sem;
use pecofence_ipc::{IpcError, mutation};
use serde_json::{Value, json};
use std::path::Path;

/// 一次语义整理最多新建多少个栅栏：防止规则包被意外改坏时把桌面铺满。
const MAX_NEW_SEMANTIC_FENCES: usize = 8;

/// 语义整理后变空的栅栏（预览和落地共用同一份判断）。
struct Emptied {
    id: FenceId,
    title: String,
    /// 指向它的规则条数：删除栅栏前会把它们改为指向「桌面」。
    rules: usize,
}

impl App {
    /// 与 `sync_desktop_if_available` 相同的枚举口径（含隐藏真实图标时的系统项）。
    pub(super) fn semantic_entries(&self) -> Vec<shell::DesktopEntry> {
        let (mut entries, _unread) = shell::enumerate_desktop_reporting();
        if self.state.config.settings.hide_real_icons && !self.no_hide_icons {
            entries.extend(shell::enumerate_special_desktop_items());
        }
        entries
    }

    /// 只计算不修改：给设置页预览用。
    pub(super) fn semantic_plan_json(&self, min_items: usize) -> Value {
        let plan = sem::plan(&self.semantic_entries(), min_items);
        let prune = self.fences_emptied_by(&plan);
        // 预览也会写一行日志：没有设置窗口时（脚本、CLI）它是唯一能看到结果的途径。
        tracing::info!(
            total = plan.total,
            moved = plan.moved(),
            groups = ?plan
                .groups
                .iter()
                .map(|g| (g.title.as_str(), g.items.len()))
                .collect::<Vec<_>>(),
            unclassified = plan.unclassified.len(),
            prune = ?prune.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
            min_items,
            "semantic plan"
        );
        json!({
            "total": plan.total,
            "moved": plan.moved(),
            "groups": plan
                .groups
                .iter()
                .map(|g| {
                    json!({
                        "title": g.title,
                        "count": g.items.len(),
                        "items": g
                            .items
                            .iter()
                            .map(|i| json!({ "name": i.name, "path": i.path }))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>(),
            "unclassified": plan
                .unclassified
                .iter()
                .map(|i| json!({ "name": i.name, "detail": i.detail }))
                .collect::<Vec<_>>(),
            "prune": prune
                .iter()
                .map(|p| json!({ "id": p.id.to_string(), "title": p.title, "rules": p.rules }))
                .collect::<Vec<_>>(),
        })
    }

    /// 语义整理后会是空的栅栏（「桌面」和文件夹门户除外）。
    ///
    /// 预览和落地共用这一个判断：面板里说「会变空」，落地时就应该真的删掉。
    fn fences_emptied_by(&self, plan: &sem::Plan) -> Vec<Emptied> {
        let moving: std::collections::HashSet<&str> = plan
            .groups
            .iter()
            .flat_map(|g| g.items.iter().map(|i| i.path.as_str()))
            .collect();
        let inbox = self.state.inbox_id();
        self.state
            .fences()
            .iter()
            .filter(|f| f.kind != FenceKind::FolderPortal && Some(f.id) != inbox)
            // 这一组要搬进去的栅栏不算：它的条目只是换了个位置。
            .filter(|f| {
                !plan
                    .groups
                    .iter()
                    .any(|g| g.title.eq_ignore_ascii_case(&f.title))
            })
            .filter(|f| {
                f.items.iter().all(|r| {
                    self.state
                        .item(r.item_id)
                        .and_then(|it| it.path.as_deref())
                        .is_some_and(|p| moving.contains(p))
                })
            })
            .map(|f| Emptied {
                id: f.id,
                title: f.title.clone(),
                rules: self
                    .state
                    .config
                    .rules
                    .list
                    .iter()
                    .filter(|r| r.target == Target::Fence(f.id))
                    .count(),
            })
            .collect()
    }

    /// 把语义分类真正落到桌面：先存快照，再建栅栏、移条目，最后清掉变空的栅栏。
    pub(super) fn semantic_apply(
        &mut self,
        min_items: usize,
        prune_empty: bool,
    ) -> std::result::Result<Value, IpcError> {
        let plan = sem::plan(&self.semantic_entries(), min_items);
        let emptied = if prune_empty {
            self.fences_emptied_by(&plan)
        } else {
            Vec::new()
        };
        if plan.is_empty() && emptied.is_empty() {
            return Ok(mutation(
                false,
                None,
                json!({
                    "moved": 0,
                    "created": 0,
                    "pruned": [],
                    "groups": [],
                    "unclassified": unclassified_json(&plan),
                }),
            ));
        }
        // 与 `items.move` 一样：这是一次布局改动，先给一条回退的路。
        let snapshot = self.auto_snapshot();
        let mut created = 0usize;
        let mut moved = 0usize;
        let mut groups = Vec::new();
        let mut near: Option<FenceId> = None;
        for group in &plan.groups {
            let (fence, is_new) = match self.semantic_fence_for(&group.title, near) {
                Some(pair) => pair,
                None => continue,
            };
            if is_new {
                created += 1;
                near = Some(fence);
            }
            let ids: Vec<ItemId> = group
                .items
                .iter()
                .filter_map(|i| self.state.item_by_path(Path::new(&i.path)))
                .collect();
            let n = self.state.move_items(&ids, fence);
            moved += n;
            groups.push(json!({
                "title": group.title,
                "count": group.items.len(),
                "moved": n,
                "created": is_new,
                "fence": fence.to_string(),
            }));
        }
        // 清掉已是空壳的默认栅栏：它们的规则改为指向「桌面」，栅栏本身删掉。
        // 规则保留在列表里（用户可改回去），快照回不了规则，所以不做删除。
        let mut pruned = Vec::new();
        for gone in &emptied {
            let mut retargeted = 0usize;
            for rule in &mut self.state.config.rules.list {
                if rule.target == Target::Fence(gone.id) {
                    rule.target = Target::Inbox;
                    retargeted += 1;
                }
            }
            if self.state.delete_fence(gone.id) {
                pruned.push(json!({
                    "title": gone.title,
                    "rules": retargeted,
                }));
            }
        }
        if moved > 0 || !pruned.is_empty() {
            self.refresh_all();
            self.schedule_save();
            self.push_settings_state();
        }
        tracing::info!(moved, created, pruned = pruned.len(), "semantic apply");
        Ok(mutation(
            moved > 0 || created > 0 || !pruned.is_empty(),
            snapshot,
            json!({
                "moved": moved,
                "created": created,
                "pruned": pruned,
                "groups": groups,
                "unclassified": unclassified_json(&plan),
            }),
        ))
    }

    /// 已有同名虚拟栅栏的 id（门户栅栏不行：它显示的是磁盘文件夹）。
    fn semantic_fence_with_title(&self, title: &str) -> Option<FenceId> {
        self.state
            .fences()
            .iter()
            .filter(|f| f.kind != FenceKind::FolderPortal)
            .find(|f| f.title.eq_ignore_ascii_case(title))
            .map(|f| f.id)
            .and_then(|id| self.state.routable_fence(id))
    }

    /// 已有同名栅栏就复用，否则在空位新建。
    fn semantic_fence_for(
        &mut self,
        title: &str,
        near: Option<FenceId>,
    ) -> Option<(FenceId, bool)> {
        if let Some(id) = self.semantic_fence_with_title(title) {
            return Some((id, false));
        }
        if self.state.fences().len() >= pecofence_core::config_store::MAX_FENCES {
            return None;
        }
        let work = self.state.work_areas.first()?;
        let (cx, cy) = ((work.left + work.right) / 2, (work.top + work.bottom) / 2);
        let rect = self.place_new_fence(3, 200.0, cx, cy, near);
        let id = self.create_fence_at(rect, Some(title))?;
        Some((id, true))
    }

    /// 新条目落进「桌面」栅栏后，按语义再分一次。
    ///
    /// 只在「新项目出现在桌面时自动归类」（`rules.keep_updated`）打开时生效，
    /// 且只处理此刻还留在 inbox 的条目——用户手动放进某个栅栏的不动，
    /// 属性规则（扩展名、类型）认得出来的也不动，语义只补规则表达不了的那部分。
    ///
    /// 一个条目就能进**已存在**的同名栅栏；要新建栅栏则至少两个条目，
    /// 否则每拖进一个文件都会长出一个只有一个图标的栅栏。
    pub(super) fn route_fresh_semantically(&mut self, fresh: &[shell::DesktopEntry]) {
        let pool: Vec<shell::DesktopEntry> = fresh
            .iter()
            .filter(|e| {
                e.origin != shell::EntryOrigin::Namespace
                    && self
                        .state
                        .item_id_for_path(&e.path)
                        .is_some_and(|id| !self.assigned_by_user(id))
            })
            .cloned()
            .collect();
        if pool.is_empty() {
            return;
        }
        let plan = sem::plan(&pool, 1);
        let mut created = 0usize;
        let mut moved = 0usize;
        let mut near: Option<FenceId> = None;
        for group in &plan.groups {
            if created >= MAX_NEW_SEMANTIC_FENCES {
                break;
            }
            let existing = self.semantic_fence_with_title(&group.title).is_some();
            if !existing && group.items.len() < sem::DEFAULT_MIN_ITEMS {
                continue;
            }
            let Some((fence, is_new)) = self.semantic_fence_for(&group.title, near) else {
                break;
            };
            if is_new {
                created += 1;
                near = Some(fence);
            }
            let ids: Vec<ItemId> = group
                .items
                .iter()
                .filter_map(|i| self.state.item_by_path(Path::new(&i.path)))
                .filter(|id| !self.assigned_by_user(*id))
                .collect();
            if ids.is_empty() {
                continue;
            }
            moved += self.state.move_items(&ids, fence);
        }
        if moved > 0 || created > 0 {
            tracing::info!(moved, created, "new desktop items routed by semantics");
            self.schedule_save();
        }
    }

    /// 条目是否被用户自己摆进某个栅栏：语义整理不碰这些。
    fn assigned_by_user(&self, id: ItemId) -> bool {
        self.state.fences().iter().any(|f| {
            f.items
                .iter()
                .any(|r| r.item_id == id && r.assigned_by == pecofence_core::AssignedBy::User)
        })
    }
}

fn unclassified_json(plan: &sem::Plan) -> Vec<Value> {
    plan.unclassified
        .iter()
        .map(|i| json!({ "name": i.name, "detail": i.detail }))
        .collect()
}
