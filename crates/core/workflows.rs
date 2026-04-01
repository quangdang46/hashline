use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::LinehashError;

const SKILLS_DIR: &str = ".linehash/skills";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkflowCatalog {
    pub root: String,
    pub packs: Vec<WorkflowPack>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkflowPack {
    pub name: String,
    pub title: String,
    pub description: String,
    pub surfaces: Vec<String>,
    pub allowed_cli_commands: Vec<String>,
    pub allowed_mcp_tools: Vec<String>,
    pub tags: Vec<String>,
    pub source: String,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct WorkflowFrontmatter {
    name: Option<String>,
    title: Option<String>,
    description: Option<String>,
    surfaces: Vec<String>,
    allowed_cli_commands: Vec<String>,
    allowed_mcp_tools: Vec<String>,
    tags: Vec<String>,
    enabled: bool,
}

impl Default for WorkflowFrontmatter {
    fn default() -> Self {
        Self {
            name: None,
            title: None,
            description: None,
            surfaces: Vec::new(),
            allowed_cli_commands: Vec::new(),
            allowed_mcp_tools: Vec::new(),
            tags: Vec::new(),
            enabled: true,
        }
    }
}

pub fn load_workflow_catalog(root: &Path) -> Result<WorkflowCatalog, LinehashError> {
    let skills_root = root.join(SKILLS_DIR);
    if !skills_root.exists() {
        return Ok(WorkflowCatalog {
            root: root.display().to_string(),
            packs: Vec::new(),
        });
    }

    let mut packs = Vec::new();
    for entry in WalkDir::new(&skills_root) {
        let entry = entry.map_err(|error| std::io::Error::other(error.to_string()))?;
        if !entry.file_type().is_file() || entry.file_name() != "SKILL.md" {
            continue;
        }

        let path = entry.into_path();
        let raw = fs::read_to_string(&path)?;
        if let Some(pack) = parse_workflow_pack(root, &path, &raw)? {
            packs.push(pack);
        }
    }

    packs.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(WorkflowCatalog {
        root: root.display().to_string(),
        packs,
    })
}

fn parse_workflow_pack(
    root: &Path,
    path: &Path,
    raw: &str,
) -> Result<Option<WorkflowPack>, LinehashError> {
    let (frontmatter, body) = split_frontmatter(path, raw)?;
    if !frontmatter.enabled {
        return Ok(None);
    }

    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/");
    let parent_name = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or("workflow-pack");
    let name = frontmatter.name.unwrap_or_else(|| parent_name.to_owned());
    let title = frontmatter.title.unwrap_or_else(|| name.replace('-', " "));
    let description = frontmatter
        .description
        .unwrap_or_else(|| "Curated linehash workflow pack for agent-driven editing.".to_owned());

    if frontmatter.allowed_cli_commands.is_empty() {
        return Err(invalid_pack(
            path,
            "expected at least one `allowed_cli_commands` entry",
        ));
    }
    if frontmatter.allowed_mcp_tools.is_empty() {
        return Err(invalid_pack(
            path,
            "expected at least one `allowed_mcp_tools` entry",
        ));
    }

    let surfaces = if frontmatter.surfaces.is_empty() {
        vec!["local".into(), "mcp".into()]
    } else {
        frontmatter.surfaces
    };

    Ok(Some(WorkflowPack {
        name,
        title,
        description,
        surfaces,
        allowed_cli_commands: frontmatter.allowed_cli_commands,
        allowed_mcp_tools: frontmatter.allowed_mcp_tools,
        tags: frontmatter.tags,
        source: relative,
        body,
    }))
}

fn split_frontmatter(
    path: &Path,
    raw: &str,
) -> Result<(WorkflowFrontmatter, String), LinehashError> {
    let normalized = raw.replace("\r\n", "\n");
    let trimmed = normalized.trim();
    let Some(rest) = trimmed.strip_prefix("---\n") else {
        return Err(invalid_pack(path, "missing TOML frontmatter"));
    };
    let Some(end) = rest.find("\n---\n") else {
        return Err(invalid_pack(path, "missing closing frontmatter delimiter"));
    };
    let (frontmatter_raw, body_with_delimiter) = rest.split_at(end);
    let frontmatter = toml::from_str(frontmatter_raw)
        .map_err(|error| invalid_pack(path, &format!("frontmatter parse failed: {error}")))?;
    let body = body_with_delimiter
        .trim_start_matches("\n---\n")
        .trim()
        .to_owned();
    if body.is_empty() {
        return Err(invalid_pack(path, "workflow body must not be empty"));
    }
    Ok((frontmatter, body))
}

fn invalid_pack(path: &Path, reason: &str) -> LinehashError {
    LinehashError::InvalidWorkflowPack {
        path: path.display().to_string(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::load_workflow_catalog;
    use tempfile::TempDir;

    #[test]
    fn loads_and_sorts_repo_skill_packs() {
        let dir = TempDir::new().unwrap();
        let alpha = dir.path().join(".linehash/skills/alpha");
        let beta = dir.path().join(".linehash/skills/beta");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&beta).unwrap();

        std::fs::write(
            alpha.join("SKILL.md"),
            concat!(
                "---\n",
                "title = \"Alpha\"\n",
                "description = \"Alpha workflow\"\n",
                "allowed_cli_commands = [\"linehash read\"]\n",
                "allowed_mcp_tools = [\"linehash_read\"]\n",
                "---\n",
                "Alpha instructions.\n",
            ),
        )
        .unwrap();
        std::fs::write(
            beta.join("SKILL.md"),
            concat!(
                "---\n",
                "title = \"Beta\"\n",
                "description = \"Beta workflow\"\n",
                "allowed_cli_commands = [\"linehash patch\"]\n",
                "allowed_mcp_tools = [\"linehash_patch\"]\n",
                "---\n",
                "Beta instructions.\n",
            ),
        )
        .unwrap();

        let catalog = load_workflow_catalog(dir.path()).unwrap();
        assert_eq!(catalog.packs.len(), 2);
        assert_eq!(catalog.packs[0].name, "alpha");
        assert_eq!(catalog.packs[1].name, "beta");
        assert_eq!(catalog.packs[0].source, ".linehash/skills/alpha/SKILL.md");
    }

    #[test]
    fn rejects_skill_packs_without_allowed_commands() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".linehash/skills/invalid");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            concat!(
                "---\n",
                "title = \"Invalid\"\n",
                "---\n",
                "No allowed commands here.\n",
            ),
        )
        .unwrap();

        let error = load_workflow_catalog(dir.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("expected at least one `allowed_cli_commands` entry")
        );
    }
}
