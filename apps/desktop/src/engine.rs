//! Everything the client reads from the local engine.
//!
//! This module is the seam. Today each function returns a fixture compiled
//! into the binary; when the daemon serves a client on this platform, the
//! bodies become calls to it and the fixtures disappear. The signatures are
//! written for that swap: no fixture type leaks into the screens.

use gpui_kit::component::tree::TreeItem;

use crate::protocol::{self, EngineStatus};

/// Whether the local engine is reachable, and what it reports when it is. The
/// fixtures below stand in for the documents it will serve; this does not.
pub fn engine_status() -> EngineStatus {
    protocol::health()
}

pub struct Project {
    pub name: &'static str,
    pub repository: &'static str,
    pub memory_count: usize,
}

pub struct MemoryDocument {
    /// Stable id, and the path shown above the preview.
    pub path: &'static str,
    pub content: &'static str,
}

/// A pending local change to one document: the published text and the draft.
pub struct DraftChange {
    pub path: &'static str,
    pub before: &'static str,
    pub after: &'static str,
}

pub fn projects() -> Vec<Project> {
    vec![
        Project {
            name: "clumsies",
            repository: "~/Projects/clumsies",
            memory_count: 12,
        },
        Project {
            name: "atlas-api",
            repository: "~/Projects/atlas-api",
            memory_count: 7,
        },
        Project {
            name: "web-console",
            repository: "~/Projects/web-console",
            memory_count: 0,
        },
    ]
}

/// The tree the engine builds from the Project's Memory refs.
pub fn memory_tree() -> Vec<TreeItem> {
    let file = |path: &'static str, label: &'static str| TreeItem::new(path, label);
    vec![
        TreeItem::new("knowledge", "knowledge")
            .expanded(true)
            .child(file("knowledge/README.md", "README.md")),
        TreeItem::new("lessons", "lessons")
            .expanded(true)
            .child(file("lessons/README.md", "README.md")),
        TreeItem::new("procedures", "procedures")
            .expanded(true)
            .child(file("procedures/README.md", "README.md"))
            .child(file("procedures/rollback.md", "部署回滚清单.md")),
        TreeItem::new("drafts", "drafts")
            .expanded(true)
            .child(file("drafts/rollback.md", "● 部署回滚清单.md")),
        TreeItem::new("skills", "skills").expanded(true).child(
            TreeItem::new("skills/project-memory", "project-memory")
                .expanded(true)
                .child(file("skills/project-memory/SKILL.md", "SKILL.md")),
        ),
    ]
}

const DOCUMENTS: [MemoryDocument; 5] = [
    MemoryDocument {
        path: "knowledge/README.md",
        content: include_str!("../../macos/Resources/MemoryStarter/knowledge/README.md"),
    },
    MemoryDocument {
        path: "lessons/README.md",
        content: include_str!("../../macos/Resources/MemoryStarter/lessons/README.md"),
    },
    MemoryDocument {
        path: "procedures/README.md",
        content: include_str!("../../macos/Resources/MemoryStarter/procedures/README.md"),
    },
    MemoryDocument {
        path: "procedures/rollback.md",
        content: include_str!("../assets/markdown-sample.md"),
    },
    MemoryDocument {
        path: "skills/project-memory/SKILL.md",
        content: include_str!("../../../packages/clumsies/skills/project-memory/SKILL.md"),
    },
];

pub fn document(path: &str) -> Option<&'static MemoryDocument> {
    DOCUMENTS.iter().find(|document| document.path == path)
}

const DRAFTS: [DraftChange; 1] = [DraftChange {
    path: "drafts/rollback.md",
    before: "\
# 部署回滚清单

当一次发布把错误版本带到线上时，按这个清单回滚。

## 步骤

1. 确认当前线上版本号
2. 切换到上一个已验证版本
3. 验证健康检查
4. 通知相关同学

## 版本对照

| 环境 | 当前版本 | 回滚目标 |
| --- | --- | --- |
| production | 2.14.0 | 2.13.3 |
| staging | 2.15.0-rc1 | 2.14.0 |

## 验证清单

- [x] 健康检查通过
- [ ] 错误率回到基线
",
    after: "\
# 部署回滚清单

当一次发布把错误版本带到线上时，按这个清单回滚。先止血，再复盘。

## 步骤

1. 确认当前线上版本号
2. 冻结发布流水线
3. 切换到上一个已验证版本
4. 验证健康检查
5. 通知相关同学，并在事故群同步

## 版本对照

| 环境 | 当前版本 | 回滚目标 |
| --- | --- | --- |
| production | 2.14.0 | 2.13.3 |
| staging | 2.15.0-rc2 | 2.14.0 |

## 验证清单

- [x] 健康检查通过
- [ ] 错误率回到基线
- [ ] 补一条事故记录
",
}];

pub fn draft(path: &str) -> Option<&'static DraftChange> {
    DRAFTS.iter().find(|draft| draft.path == path)
}
