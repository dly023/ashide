use std::path::PathBuf;

use comfy_table::Cell;
use serde::Serialize;
use warp_cli::{
    agent::OutputFormat,
    session_bridge::{
        SessionBridgeCommand, SessionBridgeEditArgs, SessionBridgeExportArgs,
        SessionBridgeForkArgs, SessionBridgeImportArgs,
    },
    GlobalOptions,
};
use warpui::{platform::TerminationMode, AppContext, ModelContext, SingletonEntity};

use crate::ai::agent_sdk::output::{self, TableFormat};
use crate::session_bridge::{
    bundle::{bundle_output_path, read_bundle, write_bundle},
    ir::SessionTimestamp,
    preview::SessionBridgePreview,
    transform::{
        edit_session, fork_session, SessionDerivation, SessionDerivationReceipt, SessionEditSpec,
    },
};

pub fn run(
    ctx: &mut AppContext,
    global_options: GlobalOptions,
    command: SessionBridgeCommand,
) -> anyhow::Result<()> {
    let runner = ctx.add_singleton_model(|_ctx| SessionBridgeCommandRunner);
    runner.update(ctx, |runner, ctx| runner.run(global_options, command, ctx))
}

struct SessionBridgeCommandRunner;

impl SessionBridgeCommandRunner {
    fn run(
        &self,
        global_options: GlobalOptions,
        command: SessionBridgeCommand,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<()> {
        run_inner(global_options, command)?;
        ctx.terminate_app(TerminationMode::ForceTerminate, None);
        Ok(())
    }
}

impl warpui::Entity for SessionBridgeCommandRunner {
    type Event = ();
}

impl SingletonEntity for SessionBridgeCommandRunner {}

#[cfg(feature = "local_fs")]
fn run_inner(global_options: GlobalOptions, command: SessionBridgeCommand) -> anyhow::Result<()> {
    match command {
        SessionBridgeCommand::List => list(global_options.output_format),
        SessionBridgeCommand::Export(args) => export(global_options.output_format, args),
        SessionBridgeCommand::Fork(args) => fork(global_options.output_format, args),
        SessionBridgeCommand::Edit(args) => edit(global_options.output_format, args),
        SessionBridgeCommand::Import(args) => import(global_options.output_format, args),
    }
}

#[cfg(not(feature = "local_fs"))]
fn run_inner(_: GlobalOptions, _: SessionBridgeCommand) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        "session-bridge requires local filesystem persistence"
    ))
}

#[cfg(feature = "local_fs")]
fn list(output_format: OutputFormat) -> anyhow::Result<()> {
    let mut conn = open_read_only_connection()?;
    let mut items = crate::session_bridge::ashide_store::list_ashide_sessions(&mut conn)?
        .into_iter()
        .map(|read_result| {
            SessionBridgeListItem::from_preview(SessionBridgePreview::from_session(
                &read_result.session,
                None,
                read_result.warnings,
            ))
        })
        .collect::<Vec<_>>();

    items.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });

    output::print_list(items, output_format);
    Ok(())
}

#[cfg(feature = "local_fs")]
fn export(output_format: OutputFormat, args: SessionBridgeExportArgs) -> anyhow::Result<()> {
    let mut conn = open_read_only_connection()?;
    let read_result =
        crate::session_bridge::ashide_store::read_ashide_session_by_id(&mut conn, &args.session)?;
    let output_path = bundle_output_path(&read_result.session, args.out.as_deref())?;
    let preview = SessionBridgePreview::from_session(
        &read_result.session,
        Some(output_path.clone()),
        read_result.warnings,
    );

    if args.dry_run {
        print_export_preview(&preview, output_format)?;
        return Ok(());
    }

    let written_path = write_bundle(&read_result.session, args.out.as_deref())?;
    output::print_list(
        [SessionBridgeExportResult::from_preview(
            preview,
            written_path,
        )],
        output_format,
    );
    Ok(())
}

#[cfg(feature = "local_fs")]
fn fork(output_format: OutputFormat, args: SessionBridgeForkArgs) -> anyhow::Result<()> {
    let mut conn = if args.dry_run {
        open_read_only_connection()?
    } else {
        crate::persistence::establish_rw_connection()?
    };
    let read_result =
        crate::session_bridge::ashide_store::read_ashide_session_by_id(&mut conn, &args.session)?;
    let derivation = fork_session(&read_result.session, args.new_session);
    write_or_preview_derivation(
        output_format,
        &mut conn,
        derivation,
        args.dry_run,
        read_result.warnings,
    )
}

#[cfg(feature = "local_fs")]
fn edit(output_format: OutputFormat, args: SessionBridgeEditArgs) -> anyhow::Result<()> {
    let mut conn = if args.dry_run {
        open_read_only_connection()?
    } else {
        crate::persistence::establish_rw_connection()?
    };
    let read_result =
        crate::session_bridge::ashide_store::read_ashide_session_by_id(&mut conn, &args.session)?;
    let spec = SessionEditSpec {
        redactions: args.redact,
        trim_after: args.trim_after,
    };
    let derivation = edit_session(&read_result.session, spec, args.new_session)?;
    write_or_preview_derivation(
        output_format,
        &mut conn,
        derivation,
        args.dry_run,
        read_result.warnings,
    )
}

#[cfg(feature = "local_fs")]
fn import(output_format: OutputFormat, args: SessionBridgeImportArgs) -> anyhow::Result<()> {
    let bundle = read_bundle(&args.bundle)?;

    let import_plan = if args.dry_run {
        let mut conn = open_read_only_connection()?;
        crate::session_bridge::ashide_store::preview_ashide_session_import(
            &mut conn,
            &bundle,
            &args.bundle,
            args.new_session,
        )?
    } else {
        let mut conn = crate::persistence::establish_rw_connection()?;
        crate::session_bridge::ashide_store::import_ashide_session_bundle(
            &mut conn,
            &bundle,
            &args.bundle,
            args.new_session,
        )?
    };

    output::print_list(
        [SessionBridgeImportResult::from_plan(
            import_plan,
            args.dry_run,
        )],
        output_format,
    );
    Ok(())
}

#[cfg(feature = "local_fs")]
fn write_or_preview_derivation(
    output_format: OutputFormat,
    conn: &mut diesel::SqliteConnection,
    derivation: SessionDerivation,
    dry_run: bool,
    warnings: Vec<String>,
) -> anyhow::Result<()> {
    let source =
        crate::session_bridge::ashide_store::SessionBridgeImportSource::from_derived_session(
            &derivation.receipt.operation,
            &derivation.receipt.source_session_id,
            &derivation.receipt.derived_session_id,
            &derivation.session,
        )?;
    let import_plan = if dry_run {
        crate::session_bridge::ashide_store::preview_ashide_session_write_back(
            conn,
            &derivation.session,
            source,
        )?
    } else {
        crate::session_bridge::ashide_store::import_ashide_session_write_back(
            conn,
            &derivation.session,
            source,
        )?
    };
    output::print_list(
        [SessionBridgeDerivationResult::from_derivation(
            derivation,
            import_plan,
            dry_run,
            warnings,
        )],
        output_format,
    );
    Ok(())
}

#[cfg(feature = "local_fs")]
fn open_read_only_connection() -> anyhow::Result<diesel::SqliteConnection> {
    let db_path = crate::persistence::database_file_path();
    let db_url = db_path.to_str().ok_or_else(|| {
        anyhow::anyhow!("database path is not valid UTF-8: {}", db_path.display())
    })?;
    crate::persistence::establish_ro_connection(db_url).map_err(Into::into)
}

fn print_export_preview(
    preview: &SessionBridgePreview,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    match output_format {
        OutputFormat::Json => output::write_json(preview, std::io::stdout()),
        OutputFormat::Ndjson => output::write_json_line(preview, std::io::stdout()),
        OutputFormat::Pretty | OutputFormat::Text => {
            println!("{}", preview.dry_run_text());
            Ok(())
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionBridgeListItem {
    session_id: String,
    title: String,
    source: String,
    project_path: Option<String>,
    message_count: usize,
    artifact_count: usize,
    updated_at: Option<String>,
    warning_count: usize,
}

impl SessionBridgeListItem {
    fn from_preview(preview: SessionBridgePreview) -> Self {
        Self {
            session_id: preview.session_id,
            title: preview.title,
            source: preview.source,
            project_path: preview.project_path,
            message_count: preview.message_count,
            artifact_count: preview.artifact_count,
            updated_at: timestamp_text(preview.updated_at.as_ref()),
            warning_count: preview.warnings.len(),
        }
    }
}

impl TableFormat for SessionBridgeListItem {
    fn header() -> Vec<Cell> {
        vec![
            Cell::new("SESSION ID"),
            Cell::new("TITLE"),
            Cell::new("PROJECT"),
            Cell::new("MESSAGES"),
            Cell::new("ARTIFACTS"),
            Cell::new("UPDATED"),
            Cell::new("WARNINGS"),
        ]
    }

    fn row(&self) -> Vec<Cell> {
        vec![
            Cell::new(&self.session_id),
            Cell::new(&self.title),
            Cell::new(self.project_path.as_deref().unwrap_or("")),
            Cell::new(self.message_count),
            Cell::new(self.artifact_count),
            Cell::new(self.updated_at.as_deref().unwrap_or("")),
            Cell::new(self.warning_count),
        ]
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionBridgeExportResult {
    session_id: String,
    title: String,
    output_path: PathBuf,
    message_count: usize,
    artifact_count: usize,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionBridgeDerivationResult {
    operation: String,
    source_session_id: String,
    derived_session_id: String,
    title: String,
    source_reference: String,
    source_sha256: String,
    dry_run: bool,
    message_count: usize,
    artifact_count: usize,
    redaction_replacement_count: usize,
    trimmed_message_count: usize,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionBridgeImportResult {
    source_session_id: String,
    target_session_id: String,
    title: String,
    project_path: Option<String>,
    source_reference: String,
    source_sha256: String,
    dry_run: bool,
    message_count: usize,
    artifact_count: usize,
}

impl SessionBridgeImportResult {
    fn from_plan(
        plan: crate::session_bridge::ashide_store::AshideSessionImportPlan,
        dry_run: bool,
    ) -> Self {
        Self {
            source_session_id: plan.source_session_id,
            target_session_id: plan.target_session_id,
            title: plan.title,
            project_path: plan.project_path,
            source_reference: plan.source_reference,
            source_sha256: plan.source_sha256,
            dry_run,
            message_count: plan.message_count,
            artifact_count: plan.artifact_count,
        }
    }
}

impl TableFormat for SessionBridgeImportResult {
    fn header() -> Vec<Cell> {
        vec![
            Cell::new("SOURCE"),
            Cell::new("TARGET"),
            Cell::new("TITLE"),
            Cell::new("PROJECT"),
            Cell::new("PROVENANCE"),
            Cell::new("DRY RUN"),
            Cell::new("MESSAGES"),
            Cell::new("ARTIFACTS"),
        ]
    }

    fn row(&self) -> Vec<Cell> {
        vec![
            Cell::new(&self.source_session_id),
            Cell::new(&self.target_session_id),
            Cell::new(&self.title),
            Cell::new(self.project_path.as_deref().unwrap_or("")),
            Cell::new(&self.source_reference),
            Cell::new(self.dry_run),
            Cell::new(self.message_count),
            Cell::new(self.artifact_count),
        ]
    }
}

impl SessionBridgeDerivationResult {
    fn from_derivation(
        derivation: SessionDerivation,
        import_plan: crate::session_bridge::ashide_store::AshideSessionImportPlan,
        dry_run: bool,
        warnings: Vec<String>,
    ) -> Self {
        let SessionDerivation {
            session,
            receipt:
                SessionDerivationReceipt {
                    operation,
                    source_session_id,
                    derived_session_id,
                    message_count,
                    artifact_count,
                    redaction_replacement_count,
                    trimmed_message_count,
                    ..
                },
        } = derivation;
        Self {
            operation,
            source_session_id,
            derived_session_id,
            title: session.title,
            source_reference: import_plan.source_reference,
            source_sha256: import_plan.source_sha256,
            dry_run,
            message_count,
            artifact_count,
            redaction_replacement_count,
            trimmed_message_count,
            warnings,
        }
    }
}

impl TableFormat for SessionBridgeDerivationResult {
    fn header() -> Vec<Cell> {
        vec![
            Cell::new("OP"),
            Cell::new("SOURCE"),
            Cell::new("DERIVED"),
            Cell::new("PROVENANCE"),
            Cell::new("DRY RUN"),
            Cell::new("MESSAGES"),
            Cell::new("ARTIFACTS"),
            Cell::new("REDACTED"),
            Cell::new("TRIMMED"),
            Cell::new("WARNINGS"),
        ]
    }

    fn row(&self) -> Vec<Cell> {
        vec![
            Cell::new(&self.operation),
            Cell::new(&self.source_session_id),
            Cell::new(&self.derived_session_id),
            Cell::new(&self.source_reference),
            Cell::new(self.dry_run),
            Cell::new(self.message_count),
            Cell::new(self.artifact_count),
            Cell::new(self.redaction_replacement_count),
            Cell::new(self.trimmed_message_count),
            Cell::new(self.warnings.len()),
        ]
    }
}

impl SessionBridgeExportResult {
    fn from_preview(preview: SessionBridgePreview, output_path: PathBuf) -> Self {
        Self {
            session_id: preview.session_id,
            title: preview.title,
            output_path,
            message_count: preview.message_count,
            artifact_count: preview.artifact_count,
            warnings: preview.warnings,
        }
    }
}

impl TableFormat for SessionBridgeExportResult {
    fn header() -> Vec<Cell> {
        vec![
            Cell::new("SESSION ID"),
            Cell::new("OUTPUT"),
            Cell::new("MESSAGES"),
            Cell::new("ARTIFACTS"),
            Cell::new("WARNINGS"),
        ]
    }

    fn row(&self) -> Vec<Cell> {
        vec![
            Cell::new(&self.session_id),
            Cell::new(self.output_path.display()),
            Cell::new(self.message_count),
            Cell::new(self.artifact_count),
            Cell::new(self.warnings.len()),
        ]
    }
}

fn timestamp_text(timestamp: Option<&SessionTimestamp>) -> Option<String> {
    timestamp.map(|timestamp| match timestamp {
        SessionTimestamp::String(value) => value.clone(),
        SessionTimestamp::Integer(value) => value.to_string(),
        SessionTimestamp::Float(value) => value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_bridge::ir::SessionIr;

    fn sample_session(session_id: &str, updated_at: Option<SessionTimestamp>) -> SessionIr {
        let mut session = SessionIr::new_ashide(session_id);
        session.title = format!("Session {session_id}");
        session.project_path = Some("/tmp/project".to_owned());
        session.updated_at = updated_at;
        session
    }

    #[test]
    fn list_item_flattens_preview_for_table_output() {
        let preview = SessionBridgePreview::from_session(
            &sample_session("abc", Some(SessionTimestamp::Integer(123))),
            None,
            vec!["warning".to_owned()],
        );

        let item = SessionBridgeListItem::from_preview(preview);

        assert_eq!(item.session_id, "abc");
        assert_eq!(item.updated_at.as_deref(), Some("123"));
        assert_eq!(item.warning_count, 1);
    }

    #[test]
    fn export_result_uses_actual_written_path() {
        let preview = SessionBridgePreview::from_session(
            &sample_session("abc", None),
            Some(PathBuf::from("/tmp/preview.json")),
            vec!["warning".to_owned()],
        );

        let result =
            SessionBridgeExportResult::from_preview(preview, PathBuf::from("/tmp/actual.json"));

        assert_eq!(result.output_path, PathBuf::from("/tmp/actual.json"));
        assert_eq!(result.warnings, vec!["warning"]);
    }
}
