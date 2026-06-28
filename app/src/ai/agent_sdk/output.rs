use std::io::Write as _;

use anyhow::Context;
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, ContentArrangement, Table};
use jaq_all::data::Runner;
use jaq_all::fmts::write::Writer;
use jaq_all::fmts::Format;
// Use jaq_json directly to ensure serde support is included.
use jaq_json::{write as jaq_write, Val};
use serde::Serialize;
use tabwriter::TabWriter;
use warp_cli::agent::OutputFormat;
use warp_cli::json_filter::JqFilter;

pub fn standard_table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table
}

/// Trait for types that can be printed as a table.
pub trait TableFormat {
    fn header() -> Vec<Cell>;

    fn row(&self) -> Vec<Cell>;
}

/// Print a list of items to stdout, respecting the `output_format`.
pub fn print_list<I, T>(items: I, output_format: OutputFormat)
where
    I: IntoIterator<Item = T>,
    T: TableFormat + Serialize,
{
    if let Err(err) = write_list(items, output_format, &mut std::io::stdout()) {
        // If we can't write to stdout, try reporting to the log file.
        log::warn!("Unable to write to stdout: {err}");
    }
}

/// Write a serializable value to `output` as pretty JSON.
pub fn write_json<T, W>(value: &T, mut output: W) -> anyhow::Result<()>
where
    T: Serialize,
    W: std::io::Write,
{
    serde_json::to_writer_pretty(&mut output, value).context("unable to write JSON output")?;
    writeln!(&mut output)?;
    Ok(())
}

/// Write a serializable value to `output` as a single-line JSON record.
pub fn write_json_line<T, W>(value: &T, mut output: W) -> anyhow::Result<()>
where
    T: Serialize,
    W: std::io::Write,
{
    serde_json::to_writer(&mut output, value).context("unable to write JSON output")?;
    writeln!(&mut output)?;
    Ok(())
}

/// Run `jq_filter` on `value` and write each output to `out` on its own line.
///
/// Top-level scalar outputs are written as raw text (see [`write_filter_output`]).
/// Runtime errors from the filter are returned as `anyhow::Error`; any outputs
/// produced before the error are still written to `out`, matching jq's behavior.
fn run_jq_filter<W: std::io::Write>(
    value: serde_json::Value,
    jq_filter: &JqFilter,
    out: &mut W,
) -> anyhow::Result<()> {
    let input_result = serde_json::from_value::<Val>(value);

    let runner = Runner {
        null_input: false,
        color_err: false,
        writer: Writer {
            format: Format::Json,
            pp: pretty_pp(),
            join: false,
        },
    };

    jaq_all::data::run(
        &runner,
        jq_filter,
        Default::default(),
        [input_result].into_iter(),
        // Callback to format invalid input errors.
        |err| anyhow::anyhow!("Invalid data: {err}"),
        // Callback to handle filter outputs.
        |result| match result {
            Ok(val) => write_filter_output(&val, out),
            Err(err) => anyhow::bail!("jq filter error: {err}"),
        },
    )?;

    Ok(())
}

/// Pretty-printer configuration used for non-scalar filter output. Matches
/// `serde_json`'s pretty printer: two-space indent, space after `:`, no
/// trailing space after `,` (since commas sit at end-of-line).
fn pretty_pp() -> jaq_write::Pp {
    jaq_write::Pp {
        indent: Some("  ".to_string()),
        sep_space: true,
        ..jaq_write::Pp::default()
    }
}

/// Write a single filter output, unwrapping top-level scalars to raw text.
///
/// - `Null` -> `null`
/// - `Bool` -> `true` / `false`
/// - `Num` -> its decimal form
/// - `TStr` / `BStr` -> the unescaped string content (no surrounding quotes)
/// - `Arr` / `Obj` -> pretty-printed JSON via `jaq_json::write`, with the same
///   formatting conventions as the non-filtered `--output-format json` path.
///
/// Every output is followed by a newline.
fn write_filter_output<W: std::io::Write>(val: &Val, out: &mut W) -> anyhow::Result<()> {
    match val {
        Val::Null => writeln!(out, "null")?,
        Val::Bool(b) => writeln!(out, "{b}")?,
        Val::Num(n) => writeln!(out, "{n}")?,
        Val::TStr(bytes) | Val::BStr(bytes) => {
            out.write_all(bytes)?;
            writeln!(out)?;
        }
        Val::Arr(_) | Val::Obj(_) => {
            jaq_write::write(&mut *out, &pretty_pp(), 0, val)
                .context("unable to write jq output as JSON")?;
            writeln!(out)?;
        }
    }
    Ok(())
}

/// Write a list of items to `output`, respecting the `output_format`.
pub fn write_list<I, T, W>(
    items: I,
    output_format: OutputFormat,
    mut output: W,
) -> anyhow::Result<()>
where
    I: IntoIterator<Item = T>,
    T: TableFormat + Serialize,
    W: std::io::Write,
{
    match output_format {
        OutputFormat::Json => {
            let items = items.into_iter().collect::<Vec<_>>();
            serde_json::to_writer(&mut output, &items).context("unable to write JSON output")
        }
        OutputFormat::Ndjson => {
            for item in items {
                write_json_line(&item, &mut output)?;
            }
            Ok(())
        }
        OutputFormat::Pretty => {
            // Use comfy-table to print a table with terminal formatting.
            let mut table = standard_table();
            table.set_header(T::header());
            for item in items {
                table.add_row(T::row(&item));
            }
            writeln!(&mut output, "{table}")?;
            Ok(())
        }
        OutputFormat::Text => {
            // Print a plain-text table.
            let mut tw = TabWriter::new(output);

            for (idx, column) in T::header().iter().enumerate() {
                if idx > 0 {
                    write!(&mut tw, "\t")?;
                }
                write!(&mut tw, "{}", column.content())?;
            }
            writeln!(&mut tw)?;

            for item in items {
                for (idx, column) in T::row(&item).iter().enumerate() {
                    if idx > 0 {
                        write!(&mut tw, "\t")?;
                    }
                    write!(&mut tw, "{}", column.content())?;
                }
                writeln!(&mut tw)?;
            }
            tw.flush()?;
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
