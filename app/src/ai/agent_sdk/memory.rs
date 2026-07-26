use anyhow::Result;
use warp_cli::{
    agent::OutputFormat,
    memory::{
        MemoryCommand, MemoryContextArgs, MemoryEvidenceArgs, MemoryPreviewArgs, MemoryRecallArgs,
        MemoryWriteArgs, RememberArgs,
    },
    GlobalOptions,
};
use warpui::{platform::TerminationMode, AppContext, ModelContext, SingletonEntity};

use crate::agent_memory::{
    build_memory_recall_preview_for_project, discover_project_root,
    recent_evidence_citations_for_project, render_memory_recall_preview_markdown, ContextAssembler,
    ContextAssemblyRequest, ContextPacket, EvidenceCitation, MemoryRecallPreview,
    MemoryRecallRequest, MemoryRecallResult, MemoryStatus, MemoryStore, MemoryWriteReceipt,
    MemoryWriteRequest,
};

pub fn remember(
    ctx: &mut AppContext,
    global_options: GlobalOptions,
    args: RememberArgs,
) -> Result<()> {
    let write_args = args.into_write_args()?;
    run_write(ctx, global_options, write_args)
}

pub fn recall(
    ctx: &mut AppContext,
    global_options: GlobalOptions,
    args: MemoryRecallArgs,
) -> Result<()> {
    run_recall(ctx, global_options, args)
}

pub fn run(
    ctx: &mut AppContext,
    global_options: GlobalOptions,
    command: MemoryCommand,
) -> Result<()> {
    let runner = ctx.add_singleton_model(|_ctx| MemoryCommandRunner);
    runner.update(ctx, |runner, ctx| runner.run(global_options, command, ctx))
}

struct MemoryCommandRunner;

impl MemoryCommandRunner {
    fn run(
        &self,
        global_options: GlobalOptions,
        command: MemoryCommand,
        ctx: &mut ModelContext<Self>,
    ) -> Result<()> {
        match command {
            MemoryCommand::Status => run_status_inner(global_options)?,
            MemoryCommand::McpServer => crate::agent_memory::mcp_server::run_stdio()?,
            MemoryCommand::Write(args) => run_write_inner(global_options, args)?,
            MemoryCommand::Recall(args) => run_recall_inner(global_options, args)?,
            MemoryCommand::Context(args) => run_context_inner(global_options, args)?,
            MemoryCommand::Preview(args) => run_preview_inner(global_options, args)?,
            MemoryCommand::Evidence(args) => run_evidence_inner(global_options, args)?,
        }
        ctx.terminate_app(TerminationMode::ForceTerminate, None);
        Ok(())
    }
}

impl warpui::Entity for MemoryCommandRunner {
    type Event = ();
}

impl SingletonEntity for MemoryCommandRunner {}

fn run_write(
    ctx: &mut AppContext,
    global_options: GlobalOptions,
    args: MemoryWriteArgs,
) -> Result<()> {
    let runner = ctx.add_singleton_model(|_ctx| MemoryCommandRunner);
    runner.update(ctx, |_, ctx| {
        run_write_inner(global_options, args)?;
        ctx.terminate_app(TerminationMode::ForceTerminate, None);
        Ok(())
    })
}

fn run_recall(
    ctx: &mut AppContext,
    global_options: GlobalOptions,
    args: MemoryRecallArgs,
) -> Result<()> {
    let runner = ctx.add_singleton_model(|_ctx| MemoryCommandRunner);
    runner.update(ctx, |_, ctx| {
        run_recall_inner(global_options, args)?;
        ctx.terminate_app(TerminationMode::ForceTerminate, None);
        Ok(())
    })
}

fn run_status_inner(global_options: GlobalOptions) -> Result<()> {
    let mut store = MemoryStore::open_discovered()?;
    let status = store.status()?;
    print_status(&status, global_options.output_format)
}

fn run_write_inner(global_options: GlobalOptions, args: MemoryWriteArgs) -> Result<()> {
    let mut store = MemoryStore::open_discovered()?;
    let receipt = store.write_memory(MemoryWriteRequest {
        kind: args.kind,
        text: args.text,
        source_ref: args.source,
        actor: "ashide-cli".to_owned(),
        source_kind: "user".to_owned(),
        confidence: "high".to_owned(),
    })?;
    print_receipt(&receipt, global_options.output_format)
}

fn run_recall_inner(global_options: GlobalOptions, args: MemoryRecallArgs) -> Result<()> {
    let mut store = MemoryStore::open_discovered()?;
    let json = args.json
        || matches!(
            global_options.output_format,
            OutputFormat::Json | OutputFormat::Ndjson
        );
    let output_format = if json {
        match global_options.output_format {
            OutputFormat::Ndjson => OutputFormat::Ndjson,
            OutputFormat::Json | OutputFormat::Pretty | OutputFormat::Text => OutputFormat::Json,
        }
    } else {
        global_options.output_format
    };
    let result = store.recall(MemoryRecallRequest {
        tier: args.tier(),
        query: args.query_text(),
        top_k: args.top_k,
    })?;
    print_recall(&result, output_format)
}

fn run_context_inner(global_options: GlobalOptions, args: MemoryContextArgs) -> Result<()> {
    let mut store = MemoryStore::open_discovered()?;
    let json = args.recall.json
        || matches!(
            global_options.output_format,
            OutputFormat::Json | OutputFormat::Ndjson
        );
    let output_format = if json {
        match global_options.output_format {
            OutputFormat::Ndjson => OutputFormat::Ndjson,
            OutputFormat::Json | OutputFormat::Pretty | OutputFormat::Text => OutputFormat::Json,
        }
    } else {
        global_options.output_format
    };
    let request = ContextAssemblyRequest {
        task: args.task,
        recall: MemoryRecallRequest {
            tier: args.recall.tier(),
            query: args.recall.query_text(),
            top_k: args.recall.top_k,
        },
        token_budget: args.token_budget,
    };
    let packet = ContextAssembler::new(&mut store).assemble(request)?;
    print_context_packet(&packet, output_format)
}

fn run_preview_inner(global_options: GlobalOptions, args: MemoryPreviewArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project_root = discover_project_root(&cwd);
    let output_format = memory_output_format(global_options.output_format, args.recall.json);
    let preview = build_memory_recall_preview_for_project(
        project_root,
        args.task,
        MemoryRecallRequest {
            tier: args.recall.tier(),
            query: args.recall.query_text(),
            top_k: args.recall.top_k,
        },
        args.token_budget,
        args.evidence_limit,
    )?;
    print_memory_preview(&preview, output_format)
}

fn run_evidence_inner(global_options: GlobalOptions, args: MemoryEvidenceArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project_root = discover_project_root(&cwd);
    let output_format = memory_output_format(global_options.output_format, args.json);
    let citations = recent_evidence_citations_for_project(project_root, args.limit)?;
    print_evidence_citations(&citations, output_format)
}

fn memory_output_format(output_format: OutputFormat, force_json: bool) -> OutputFormat {
    if force_json || matches!(output_format, OutputFormat::Json | OutputFormat::Ndjson) {
        match output_format {
            OutputFormat::Ndjson => OutputFormat::Ndjson,
            OutputFormat::Json | OutputFormat::Pretty | OutputFormat::Text => OutputFormat::Json,
        }
    } else {
        output_format
    }
}

fn print_status(status: &MemoryStatus, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json => crate::ai::agent_sdk::output::write_json(status, std::io::stdout()),
        OutputFormat::Ndjson => {
            crate::ai::agent_sdk::output::write_json_line(status, std::io::stdout())
        }
        OutputFormat::Pretty | OutputFormat::Text => {
            println!("Memory: {}", status.mode);
            println!("root: {}", status.memory_root.display());
            println!("database: {}", status.database_path.display());
            println!("events: {}", status.event_count);
            println!("memories: {}", status.memory_count);
            println!("fts: {}", if status.fts_enabled { "ok" } else { "missing" });
            Ok(())
        }
    }
}

fn print_receipt(receipt: &MemoryWriteReceipt, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json => crate::ai::agent_sdk::output::write_json(receipt, std::io::stdout()),
        OutputFormat::Ndjson => {
            crate::ai::agent_sdk::output::write_json_line(receipt, std::io::stdout())
        }
        OutputFormat::Pretty | OutputFormat::Text => {
            println!(
                "Remembered [{}/high] event={} memory={}",
                receipt.kind, receipt.event_id, receipt.memory_id
            );
            if let Some(source_ref) = receipt.source_ref.as_deref() {
                println!("source: {source_ref}");
            }
            println!("store: {}", receipt.memory_root.display());
            Ok(())
        }
    }
}

fn print_recall(result: &MemoryRecallResult, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json => crate::ai::agent_sdk::output::write_json(result, std::io::stdout()),
        OutputFormat::Ndjson => {
            for memory in &result.memories {
                crate::ai::agent_sdk::output::write_json_line(memory, std::io::stdout())?;
            }
            Ok(())
        }
        OutputFormat::Pretty | OutputFormat::Text => {
            println!(
                "Recall: {}, {} hits, {} conflicts, {} stale-risk",
                result.tier, result.hit_count, result.conflicts, result.stale_risk
            );
            for (idx, memory) in result.memories.iter().enumerate() {
                println!(
                    "\n{}. [{}/{}] {}",
                    idx + 1,
                    memory.kind,
                    memory.confidence,
                    memory.text
                );
                if let Some(source_ref) = memory.source_ref.as_deref() {
                    println!("   source: {source_ref}");
                }
                println!("   event: {}", memory.event_id);
            }
            print_memory_citations(&result.citations);
            Ok(())
        }
    }
}

fn print_context_packet(packet: &ContextPacket, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json => crate::ai::agent_sdk::output::write_json(packet, std::io::stdout()),
        OutputFormat::Ndjson => {
            for block in &packet.blocks {
                crate::ai::agent_sdk::output::write_json_line(block, std::io::stdout())?;
            }
            Ok(())
        }
        OutputFormat::Pretty | OutputFormat::Text => {
            println!(
                "ContextPacket: {} blocks, used {}/{} tokens",
                packet.blocks.len(),
                packet.token_budget.used,
                packet.token_budget.requested
            );
            if let Some(task) = packet.task.as_deref() {
                println!("task: {task}");
            }
            for block in &packet.blocks {
                println!("\n## {}", block.label);
                println!("{}", block.content);
            }
            print_memory_citations(&packet.memory_citations);
            if !packet.caveats.is_empty() {
                println!("\nCaveats:");
                for caveat in &packet.caveats {
                    println!("- {caveat}");
                }
            }
            Ok(())
        }
    }
}

fn print_memory_preview(preview: &MemoryRecallPreview, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json => crate::ai::agent_sdk::output::write_json(preview, std::io::stdout()),
        OutputFormat::Ndjson => {
            if let Some(packet) = preview.context_packet.as_ref() {
                for block in &packet.blocks {
                    crate::ai::agent_sdk::output::write_json_line(block, std::io::stdout())?;
                }
            }
            for citation in &preview.evidence_citations {
                crate::ai::agent_sdk::output::write_json_line(citation, std::io::stdout())?;
            }
            Ok(())
        }
        OutputFormat::Pretty | OutputFormat::Text => {
            print!("{}", render_memory_recall_preview_markdown(preview));
            Ok(())
        }
    }
}

fn print_evidence_citations(
    citations: &[EvidenceCitation],
    output_format: OutputFormat,
) -> Result<()> {
    match output_format {
        OutputFormat::Json => {
            crate::ai::agent_sdk::output::write_json(&citations.to_vec(), std::io::stdout())
        }
        OutputFormat::Ndjson => {
            for citation in citations {
                crate::ai::agent_sdk::output::write_json_line(citation, std::io::stdout())?;
            }
            Ok(())
        }
        OutputFormat::Pretty | OutputFormat::Text => {
            print_evidence_citation_list(citations);
            Ok(())
        }
    }
}

fn print_memory_citations(citations: &[crate::agent_memory::MemoryCitation]) {
    if citations.is_empty() {
        return;
    }

    println!("\nCitations:");
    for citation in citations {
        let source = citation.source_ref.as_deref().unwrap_or("unknown-source");
        println!(
            "- {} [{}] source={} memory={}",
            citation.citation_id, citation.kind, source, citation.memory_id
        );
    }
}

fn print_evidence_citation_list(citations: &[EvidenceCitation]) {
    if citations.is_empty() {
        return;
    }

    println!("\nEvidence citations:");
    for citation in citations {
        let source = citation
            .source_paths
            .first()
            .map(String::as_str)
            .unwrap_or("unknown-source");
        println!(
            "- {} [{}:{}] source={} summary={}",
            citation.citation_id,
            citation.tool_name,
            citation.trusted_level,
            source,
            citation.output_summary
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use warp_cli::memory::MemoryKind;

    #[test]
    fn recall_json_flag_promotes_pretty_output_to_json() {
        let args = MemoryRecallArgs {
            fast: false,
            focused: false,
            full: false,
            top_k: 8,
            json: true,
            query: vec!["SQLite".to_owned()],
        };
        assert!(args.json);
        assert_eq!(args.tier().as_str(), "focused");
    }

    #[test]
    fn remember_args_map_to_write_request_shape() {
        let args = RememberArgs {
            words: vec!["decision".to_owned(), "Do not mutate AGENTS.md".to_owned()],
        };
        let write = args.into_write_args().unwrap();
        assert_eq!(write.kind, MemoryKind::Decision);
        assert_eq!(write.text, "Do not mutate AGENTS.md");
    }
}
