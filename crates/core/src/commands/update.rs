use std::io::Write;

use crate::cli::UpdateCmd;
use crate::context::{CommandContext, OutputMode};
use crate::error::HashlineError;
use crate::update::{self, Release, UpdateStatus};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: UpdateCmd,
) -> Result<(), HashlineError> {
    let installed = update::current_version().to_string();

    // Resolve the release to act on. An explicit --version skips the
    // network lookup entirely (and enables downgrades).
    let release = match &cmd.version {
        Some(version) => Release::from_version(version),
        None => update::fetch_latest_release()
            .map_err(|message| HashlineError::UpdateFailed { message })?,
    };
    update::record_release_seen(&release.version);

    let status = update::compare_status(&installed, &release.version);

    if cmd.check {
        return emit_status(ctx, &installed, &release.version, status);
    }

    // Install path: an explicit --version always installs; otherwise only
    // a strictly newer release is installed.
    let explicit = cmd.version.is_some();
    if !explicit && !matches!(status, UpdateStatus::Outdated { .. }) {
        return emit_status(ctx, &installed, &release.version, UpdateStatus::UpToDate);
    }
    if matches!(status, UpdateStatus::Unknown) {
        return Err(HashlineError::UpdateFailed {
            message: format!(
                "cannot compare installed version '{installed}' with release '{}'",
                release.version
            ),
        });
    }

    let report = update::download_and_install(&release)
        .map_err(|message| HashlineError::UpdateFailed { message })?;
    if !report.checksum_verified {
        writeln!(
            ctx.stderr(),
            "NOTE checksum verification skipped (release publishes no .sha256 asset)"
        )?;
    }

    match ctx.output_mode() {
        OutputMode::Json => {
            let output = serde_json::json!({
                "success": true,
                "status": "updated",
                "previous": report.previous,
                "current": report.current,
                "path": report.path.display().to_string(),
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
        }
        OutputMode::Verbose => {
            writeln!(
                ctx.stdout(),
                "updated hashline {} -> {} at {}",
                report.previous,
                report.current,
                report.path.display()
            )?;
        }
        OutputMode::Compact | OutputMode::Ndjson => {
            writeln!(
                ctx.stdout(),
                "OK update status=updated previous={} current={} path={}",
                report.previous,
                report.current,
                report.path.display()
            )?;
        }
    }

    Ok(())
}

fn emit_status<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    installed: &str,
    latest: &str,
    status: UpdateStatus,
) -> Result<(), HashlineError> {
    match (ctx.output_mode(), status) {
        (OutputMode::Json, UpdateStatus::UpToDate) => {
            let output = serde_json::json!({
                "success": true,
                "status": "up-to-date",
                "current": installed,
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
        }
        (OutputMode::Json, UpdateStatus::Outdated { .. }) => {
            let output = serde_json::json!({
                "success": true,
                "status": "available",
                "installed": installed,
                "latest": latest,
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
        }
        (OutputMode::Json, UpdateStatus::Unknown) => {
            let output = serde_json::json!({
                "success": true,
                "status": "unknown",
                "installed": installed,
                "latest": latest,
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
        }
        (OutputMode::Verbose, UpdateStatus::UpToDate) => {
            writeln!(ctx.stdout(), "hashline is up to date (version {installed})")?;
        }
        (OutputMode::Verbose, UpdateStatus::Outdated { .. }) => {
            writeln!(
                ctx.stdout(),
                "a new hashline release is available: {latest} (installed {installed})"
            )?;
        }
        (OutputMode::Verbose, UpdateStatus::Unknown) => {
            writeln!(
                ctx.stdout(),
                "could not compare installed version '{installed}' with '{latest}'"
            )?;
        }
        (OutputMode::Compact | OutputMode::Ndjson, UpdateStatus::UpToDate) => {
            writeln!(
                ctx.stdout(),
                "OK update status=up-to-date version={installed}"
            )?;
        }
        (OutputMode::Compact | OutputMode::Ndjson, UpdateStatus::Outdated { .. }) => {
            writeln!(
                ctx.stdout(),
                "OK update status=available installed={installed} latest={latest}"
            )?;
        }
        (OutputMode::Compact | OutputMode::Ndjson, UpdateStatus::Unknown) => {
            writeln!(
                ctx.stdout(),
                "OK update status=unknown installed={installed} latest={latest}"
            )?;
        }
    }
    Ok(())
}
