//! Semantic text runs for native text/grid adapters. No terminal escape bytes.
use crate::{external_link, ContentDocument, ContentKind as K, ContentNode};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceSpan>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub source_grapheme: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<BackgroundRole>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strike: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub code: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<u8>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan { pub start: usize, pub end: usize }
/// Theme-owned colors, never an untrusted terminal control or platform RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundRole {
    MessageHeader,
    MessageBody,
    Reference,
}
fn is_zero(value: &usize) -> bool { *value == 0 }
fn is_false(value: &bool) -> bool { !*value }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRun {
    pub text: String,
    pub style: TextStyle,
}

pub fn screen_runs(doc: &ContentDocument) -> Vec<TextRun> {
    screen_runs_for_width(doc, 0)
}

pub fn screen_runs_for_width(doc: &ContentDocument, columns: usize) -> Vec<TextRun> {
    let mut runs = Vec::new();
    for node in &doc.nodes {
        render(node, &TextStyle::default(), &mut runs, 0, columns);
    }
    runs
}
pub fn push_run(runs: &mut Vec<TextRun>, text: &str, style: &TextStyle) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut().filter(|r| r.style == *style) {
        last.text.push_str(text);
    } else {
        runs.push(TextRun {
            text: text.into(),
            style: style.clone(),
        });
    }
}
fn render(node: &ContentNode, style: &TextStyle, out: &mut Vec<TextRun>, depth: usize, columns: usize) {
    let mut style = style.clone();
    style.source = Some(SourceSpan { start: node.start, end: node.end });
    match node.kind {
        K::Heading | K::Strong => style.bold = true,
        K::Emphasis => style.italic = true,
        K::Strike => style.strike = true,
        K::Code | K::CodeBlock => style.code = true,
        K::Link => style.link = node.target.clone().filter(|s| external_link(s)),
        K::Quote => {
            style.dim = true;
            push_run(out, "│ ", &style);
        }
        K::Image => push_run(out, "[Image: ", &style),
        K::Task => push_run(
            out,
            if node.checked == Some(true) {
                "[x] "
            } else {
                "[ ] "
            },
            &style,
        ),
        K::SoftBreak | K::HardBreak => push_run(out, "\n", &style),
        K::Rule => push_run(out, "────────\n", &style),
        _ => {}
    }
    if matches!(
        node.kind,
        K::Text | K::Code | K::Math | K::Html | K::FootnoteReference
    ) {
        push_run(out, &node.text, &style);
    }
    if node.kind == K::Table {
        if columns > 0 && render_table(node, &style, out, columns) { return; }
        let headers = node.children.iter().find(|n| n.kind == K::TableHead);
        for row in node.children.iter().filter(|n| n.kind == K::TableRow) {
            for (i, cell) in row.children.iter().enumerate() {
                if let Some(header) = headers.and_then(|h| h.children.get(i)) {
                    for child in &header.children {
                        render(child, &style, out, depth, columns);
                    }
                    push_run(out, ": ", &style);
                }
                for child in &cell.children {
                    render(child, &style, out, depth, columns);
                }
                push_run(out, "\n", &style);
            }
            push_run(out, "\n", &style);
        }
        return;
    }
    for (i, child) in node.children.iter().enumerate() {
        if node.kind == K::List {
            let marker = node.ordinal.map_or_else(
                || "• ".into(),
                |n| format!("{}. ", n.saturating_add(i as u64)),
            );
            push_run(out, &format!("{}{}", "  ".repeat(depth), marker), &style);
        }
        render(
            child,
            &style,
            out,
            depth + usize::from(node.kind == K::List),
            columns,
        );
    }
    match node.kind {
        K::Paragraph | K::Heading | K::CodeBlock | K::Quote => {
            push_run(out, "\n\n", &TextStyle::default())
        }
        K::Item => push_run(out, "\n", &TextStyle::default()),
        K::Image => push_run(out, "]", &style),
        _ => {}
    }
}

fn render_table(node: &ContentNode, style: &TextStyle, out: &mut Vec<TextRun>, columns: usize) -> bool {
    use unicode_width::UnicodeWidthStr;
    let rows: Vec<_> = node.children.iter().filter(|n| matches!(n.kind, K::TableHead | K::TableRow)).collect();
    let count = rows.iter().map(|r| r.children.len()).max().unwrap_or(0);
    if count == 0 { return false; }
    let mut widths = vec![0; count];
    let cells: Vec<Vec<Vec<TextRun>>> = rows.iter().map(|row| row.children.iter().enumerate().map(|(i, cell)| {
        let mut runs = Vec::new();
        for child in &cell.children { render(child, style, &mut runs, 0, 0); }
        let text = runs.iter().map(|r| r.text.as_str()).collect::<String>();
        widths[i] = widths[i].max(text.width());
        runs
    }).collect()).collect();
    if widths.iter().sum::<usize>() + (count - 1) * 3 > columns { return false; }
    let muted = TextStyle { dim: true, ..TextStyle::default() };
    for (row, cells) in rows.iter().zip(cells) {
        for (i, cell) in cells.into_iter().enumerate() {
            if i > 0 { push_run(out, " │ ", &muted); }
            let width = cell.iter().map(|r| r.text.as_str()).collect::<String>().width();
            for mut run in cell { run.style.bold |= row.kind == K::TableHead; push_run(out, &run.text, &run.style); }
            push_run(out, &" ".repeat(widths[i].saturating_sub(width)), style);
        }
        push_run(out, "\n", style);
        if row.kind == K::TableHead {
            for (i, width) in widths.iter().enumerate() {
                if i > 0 { push_run(out, "─┼─", &muted); }
                push_run(out, &"─".repeat(*width), &muted);
            }
            push_run(out, "\n", style);
        }
    }
    true
}


