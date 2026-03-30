use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const SERVER_NAME: &str = "linehash";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallStatus {
    Installed,
    Updated,
    Unchanged,
}

#[derive(Debug)]
pub struct InstallOutcome {
    pub host: &'static str,
    pub path: PathBuf,
    pub status: InstallStatus,
    pub note: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
enum ConfigFormat {
    Json { servers_key: &'static str },
    Toml,
}

#[derive(Debug)]
struct HostInfo {
    name: &'static str,
    path: PathBuf,
    format: ConfigFormat,
    note: Option<&'static str>,
}

pub fn auto_install(cwd: &Path) -> Result<Vec<InstallOutcome>, String> {
    install_hosts(&detect_hosts(cwd)?, cwd)
}

pub fn run_install_mcp<W: Write, E: Write>(
    cwd: &Path,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), String> {
    let outcomes = auto_install(cwd)?;
    if outcomes.is_empty() {
        writeln!(
            stderr,
            "No supported MCP providers detected. Skipped auto-install."
        )
        .map_err(|error| error.to_string())?;
        return Ok(());
    }

    writeln!(stdout, "linehash MCP auto-install results:").map_err(|error| error.to_string())?;
    for outcome in outcomes {
        let status = match outcome.status {
            InstallStatus::Installed => "installed",
            InstallStatus::Updated => "updated",
            InstallStatus::Unchanged => "unchanged",
        };
        writeln!(
            stdout,
            "- {}: {} ({})",
            outcome.host,
            status,
            outcome.path.display()
        )
        .map_err(|error| error.to_string())?;
        if let Some(note) = outcome.note {
            writeln!(stdout, "  {}", note).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn install_hosts(hosts: &[&'static str], cwd: &Path) -> Result<Vec<InstallOutcome>, String> {
    let mut outcomes = Vec::new();
    for &host in hosts {
        outcomes.push(install_host(host, cwd)?);
    }
    Ok(outcomes)
}

fn detect_hosts(cwd: &Path) -> Result<Vec<&'static str>, String> {
    if let Ok(forced) = std::env::var("LINEHASH_MCP_HOST") {
        let hosts = forced
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map(resolve_host_name)
            .collect::<Result<Vec<_>, _>>()?;
        if !hosts.is_empty() {
            return Ok(hosts);
        }
    }

    let home = home_dir()?;
    let mut detected = Vec::new();

    if home.join(".codex").is_dir() {
        detected.push("codex");
    }
    if home.join(".claude.json").exists() {
        detected.push("claude-code");
    }
    if home.join(".cursor").is_dir() {
        detected.push("cursor");
    }
    if home.join(".codeium/windsurf").is_dir() {
        detected.push("windsurf");
    }
    if cwd.join(".vscode").is_dir() {
        detected.push("vscode");
    }
    if home.join(".gemini").is_dir() {
        detected.push("gemini");
    }
    if home.join(".opencode.json").exists() {
        detected.push("opencode");
    }
    if home.join(".config/amp").is_dir() {
        detected.push("amp");
    }
    if home.join(".factory").is_dir() {
        detected.push("droid");
    }

    Ok(detected)
}

fn resolve_host_name(host: &str) -> Result<&'static str, String> {
    match host {
        "claude-code" => Ok("claude-code"),
        "cursor" => Ok("cursor"),
        "windsurf" => Ok("windsurf"),
        "vscode" => Ok("vscode"),
        "gemini" => Ok("gemini"),
        "opencode" => Ok("opencode"),
        "codex" => Ok("codex"),
        "amp" => Ok("amp"),
        "droid" => Ok("droid"),
        _ => Err(format!("unknown MCP host override: {host}")),
    }
}

fn install_host(host: &'static str, cwd: &Path) -> Result<InstallOutcome, String> {
    let info = resolve_host(host, cwd)?;
    if let Some(parent) = info.path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    let status = match info.format {
        ConfigFormat::Json { servers_key } => write_json_config(&info.path, servers_key)?,
        ConfigFormat::Toml => write_toml_config(&info.path)?,
    };

    Ok(InstallOutcome {
        host: info.name,
        path: info.path,
        status,
        note: info.note,
    })
}

fn resolve_host(host: &'static str, cwd: &Path) -> Result<HostInfo, String> {
    let home = home_dir()?;
    match host {
        "claude-code" => Ok(HostInfo {
            name: host,
            path: home.join(".claude.json"),
            format: ConfigFormat::Json {
                servers_key: "mcpServers",
            },
            note: Some("User scope."),
        }),
        "cursor" => Ok(HostInfo {
            name: host,
            path: home.join(".cursor/mcp.json"),
            format: ConfigFormat::Json {
                servers_key: "mcpServers",
            },
            note: Some("Global scope."),
        }),
        "windsurf" => Ok(HostInfo {
            name: host,
            path: home.join(".codeium/windsurf/mcp_config.json"),
            format: ConfigFormat::Json {
                servers_key: "mcpServers",
            },
            note: Some("Global scope."),
        }),
        "vscode" => Ok(HostInfo {
            name: host,
            path: cwd.join(".vscode/mcp.json"),
            format: ConfigFormat::Json {
                servers_key: "servers",
            },
            note: Some("Project scope."),
        }),
        "gemini" => Ok(HostInfo {
            name: host,
            path: home.join(".gemini/settings.json"),
            format: ConfigFormat::Json {
                servers_key: "mcpServers",
            },
            note: Some("User scope."),
        }),
        "opencode" => Ok(HostInfo {
            name: host,
            path: home.join(".opencode.json"),
            format: ConfigFormat::Json {
                servers_key: "mcpServers",
            },
            note: Some("User scope."),
        }),
        "codex" => Ok(HostInfo {
            name: host,
            path: home.join(".codex/config.toml"),
            format: ConfigFormat::Toml,
            note: Some("User scope."),
        }),
        "amp" => Ok(HostInfo {
            name: host,
            path: home.join(".config/amp/settings.json"),
            format: ConfigFormat::Json {
                servers_key: "amp.mcpServers",
            },
            note: Some("User scope."),
        }),
        "droid" => Ok(HostInfo {
            name: host,
            path: home.join(".factory/mcp.json"),
            format: ConfigFormat::Json {
                servers_key: "mcpServers",
            },
            note: Some("User scope."),
        }),
        _ => Err(format!("unsupported MCP host: {host}")),
    }
}

fn write_json_config(path: &Path, servers_key: &str) -> Result<InstallStatus, String> {
    let entry = server_entry();
    let mut config = if path.exists() {
        let raw = fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        serde_json::from_str::<Value>(&raw)
            .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))?
    } else {
        json!({})
    };

    let status = upsert_json_server(&mut config, servers_key, entry)?;
    let rendered =
        serde_json::to_string_pretty(&config).expect("serde_json::Value is always serializable");
    fs::write(path, rendered)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(status)
}

fn write_toml_config(path: &Path) -> Result<InstallStatus, String> {
    let (command, args) = command_and_args();
    let command = command.replace('\\', "\\\\");
    let args = args
        .iter()
        .map(|arg| format!("\"{}\"", arg.replace('\\', "\\\\")))
        .collect::<Vec<_>>()
        .join(", ");
    let section =
        format!("[mcp_servers.{SERVER_NAME}]\ncommand = \"{command}\"\nargs = [{args}]\n");

    let existing = if path.exists() {
        fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?
    } else {
        String::new()
    };

    let header = format!("[mcp_servers.{SERVER_NAME}]");
    let status = if let Some(start) = existing.find(&header) {
        let rest = &existing[start..];
        let end = rest[1..]
            .find("\n[")
            .map_or(existing.len(), |index| start + 1 + index + 1);
        let current = &existing[start..end];
        if current.trim() == section.trim() {
            return Ok(InstallStatus::Unchanged);
        }
        let updated = format!("{}{}{}", &existing[..start], section, &existing[end..]);
        fs::write(path, updated)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        InstallStatus::Updated
    } else {
        let separator = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let updated = format!("{existing}{separator}\n{section}");
        fs::write(path, updated)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        InstallStatus::Installed
    };

    Ok(status)
}

fn server_entry() -> Value {
    let (command, args) = command_and_args();
    json!({
        "command": command,
        "args": args,
    })
}

fn command_and_args() -> (String, Vec<String>) {
    let command = std::env::current_exe()
        .ok()
        .map(resolve_command_path)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "linehash".to_owned());
    (command, vec!["mcp".to_owned()])
}

fn resolve_command_path(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

fn upsert_json_server(
    config: &mut Value,
    servers_key: &str,
    entry: Value,
) -> Result<InstallStatus, String> {
    let root = config
        .as_object_mut()
        .ok_or("config root is not a JSON object")?;
    let servers = root
        .entry(servers_key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("{servers_key} is not a JSON object"))?;

    let status = match servers.get(SERVER_NAME) {
        None => InstallStatus::Installed,
        Some(existing) if *existing == entry => InstallStatus::Unchanged,
        Some(_) => InstallStatus::Updated,
    };
    servers.insert(SERVER_NAME.into(), entry);
    Ok(status)
}

fn home_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .map_err(|_| "USERPROFILE not set".into())
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .map_err(|_| "HOME not set".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{InstallStatus, SERVER_NAME, resolve_command_path, upsert_json_server};
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn inserts_json_server_when_missing() {
        let mut config = json!({});
        let status = upsert_json_server(
            &mut config,
            "mcpServers",
            json!({"command": "linehash", "args": ["mcp"]}),
        )
        .unwrap();

        assert_eq!(status, InstallStatus::Installed);
        assert_eq!(
            config["mcpServers"][SERVER_NAME]["command"],
            json!("linehash")
        );
    }

    #[test]
    fn keeps_literal_dotted_json_keys() {
        let mut config = json!({});
        upsert_json_server(
            &mut config,
            "amp.mcpServers",
            json!({"command": "linehash", "args": ["mcp"]}),
        )
        .unwrap();

        assert!(config.get("amp.mcpServers").is_some());
        assert!(config.get("amp").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn canonicalizes_symlinked_command_path() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let target = dir.path().join("linehash-real");
        let link = dir.path().join("linehash-link");
        std::fs::write(&target, "binary").unwrap();
        symlink(&target, &link).unwrap();

        assert_eq!(resolve_command_path(link), target);
    }
}
