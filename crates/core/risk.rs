use serde::Serialize;

use crate::cli::Commands;
use crate::error::LinehashError;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Medium,
    High,
    Blocked,
}

impl RiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Medium => "medium",
            Self::High => "high",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RiskReason {
    pub code: &'static str,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RiskAssessment {
    pub operation: &'static str,
    pub level: RiskLevel,
    pub summary: String,
    pub reasons: Vec<RiskReason>,
}

pub fn assess_command(command: &Commands) -> Option<RiskAssessment> {
    match command {
        Commands::Delete(cmd) => {
            let is_range = cmd.anchor.contains("..");
            Some(RiskAssessment {
                operation: "delete",
                level: if is_range {
                    RiskLevel::High
                } else {
                    RiskLevel::Medium
                },
                summary: if is_range {
                    "Delete will remove a resolved line range and permanently drop content.".into()
                } else {
                    "Delete will permanently remove one resolved line.".into()
                },
                reasons: vec![
                    RiskReason {
                        code: "content_loss",
                        message: "Deleted content is not preserved unless you dry-run first or keep an audit trail.".into(),
                    },
                    RiskReason {
                        code: "anchor_sensitive",
                        message: "The command depends on anchors resolving exactly against the current file state.".into(),
                    },
                ],
            })
        }
        Commands::Move(_) => Some(RiskAssessment {
            operation: "move",
            level: RiskLevel::Medium,
            summary: "Move will reorder existing lines and can shift surrounding anchors.".into(),
            reasons: vec![
                RiskReason {
                    code: "structural_reorder",
                    message: "Reordering content can invalidate later anchor assumptions in the same file.".into(),
                },
                RiskReason {
                    code: "dual_anchor",
                    message: "Both source and target anchors must resolve cleanly for the move to stay safe.".into(),
                },
            ],
        }),
        Commands::Patch(_) => Some(RiskAssessment {
            operation: "patch",
            level: RiskLevel::High,
            summary: "Patch can batch multiple edits, inserts, and deletes in one transaction.".into(),
            reasons: vec![
                RiskReason {
                    code: "multi_step_mutation",
                    message: "A patch can rewrite several regions at once, so review the planned operations carefully.".into(),
                },
                RiskReason {
                    code: "content_loss",
                    message: "Patch operations may delete or replace content across multiple anchors.".into(),
                },
            ],
        }),
        _ => None,
    }
}

pub fn blocked_assessment(error: &LinehashError) -> Option<RiskAssessment> {
    match error {
        LinehashError::AmbiguousHash { .. } => Some(RiskAssessment {
            operation: "anchor_resolution",
            level: RiskLevel::Blocked,
            summary: "The mutation was blocked because the target hash is ambiguous.".into(),
            reasons: vec![RiskReason {
                code: "ambiguous_anchor",
                message: "Use a line-qualified anchor so linehash does not guess which matching line you meant.".into(),
            }],
        }),
        LinehashError::HashNotFound { .. } => Some(RiskAssessment {
            operation: "anchor_resolution",
            level: RiskLevel::Blocked,
            summary: "The mutation was blocked because the target hash no longer exists in the file.".into(),
            reasons: vec![RiskReason {
                code: "missing_anchor",
                message: "Re-read the file to collect a fresh anchor before retrying the destructive operation.".into(),
            }],
        }),
        LinehashError::StaleAnchor { .. } | LinehashError::StaleFile { .. } => {
            Some(RiskAssessment {
                operation: "anchor_resolution",
                level: RiskLevel::Blocked,
                summary: "The mutation was blocked because the file changed since the last anchor read.".into(),
                reasons: vec![RiskReason {
                    code: "stale_state",
                    message: "Linehash refuses to apply destructive changes when the file state has drifted.".into(),
                }],
            })
        }
        LinehashError::PatchFailed { reason, .. } => Some(RiskAssessment {
            operation: "patch",
            level: RiskLevel::Blocked,
            summary: "The patch transaction was blocked before linehash could safely apply every operation.".into(),
            reasons: vec![RiskReason {
                code: "patch_validation",
                message: format!("Patch validation failed: {reason}"),
            }],
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{assess_command, blocked_assessment, RiskLevel};
    use crate::cli::{Commands, DeleteCmd, MoveCmd, MoveDirection, PatchCmd};
    use crate::error::LinehashError;
    use std::path::PathBuf;

    #[test]
    fn delete_range_is_high_risk() {
        let assessment = assess_command(&Commands::Delete(DeleteCmd {
            file: PathBuf::from("demo.txt"),
            anchor: "1:aa..3:cc".into(),
            dry_run: false,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            json: false,
        }))
        .unwrap();

        assert_eq!(assessment.level, RiskLevel::High);
        assert_eq!(assessment.operation, "delete");
    }

    #[test]
    fn move_is_medium_risk() {
        let assessment = assess_command(&Commands::Move(MoveCmd {
            file: PathBuf::from("demo.txt"),
            anchor: "1:aa".into(),
            direction: MoveDirection::After,
            target: "3:cc".into(),
            dry_run: false,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
        }))
        .unwrap();

        assert_eq!(assessment.level, RiskLevel::Medium);
        assert_eq!(assessment.operation, "move");
    }

    #[test]
    fn patch_defaults_to_high_risk() {
        let assessment = assess_command(&Commands::Patch(PatchCmd {
            file: PathBuf::from("demo.txt"),
            patch: "ops.json".into(),
            dry_run: false,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            json: false,
        }))
        .unwrap();

        assert_eq!(assessment.level, RiskLevel::High);
        assert_eq!(assessment.operation, "patch");
    }

    #[test]
    fn stale_anchor_maps_to_blocked_risk() {
        let assessment = blocked_assessment(&LinehashError::StaleAnchor {
            anchor: "2:aa".into(),
            line: 2,
            expected: "aa".into(),
            actual: "bb".into(),
            path: "demo.txt".into(),
            relocated_suffix: "".into(),
        })
        .unwrap();

        assert_eq!(assessment.level, RiskLevel::Blocked);
        assert_eq!(assessment.operation, "anchor_resolution");
    }
}
