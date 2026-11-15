//! Device-independent, bounded content structure. No layout or resource fetching.
use crate::{ProjectionError, SourceFormat, MAXIMUM_SOURCE_BYTES};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Paragraph,
    Heading,
    Quote,
    List,
    Item,
    CodeBlock,
    HtmlBlock,
    Table,
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strike,
    Link,
    Image,
    Text,
    Code,
    Math,
    Html,
    SoftBreak,
    HardBreak,
    Rule,
    Task,
    Footnote,
    FootnoteReference,
    DefinitionList,
    DefinitionTitle,
    Definition,
    Superscript,
    Subscript,
    Metadata,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentNode {
    pub kind: ContentKind,
    /// UTF-8 offsets into the immutable original source, not display offsets.
    pub start: usize,
    pub end: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ContentNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentDocument {
    pub version: u8,
    pub nodes: Vec<ContentNode>,
}

/// Navigation is a separate user intent. Never treat arbitrary URI schemes as commands.
pub fn external_link(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && target
            .split_once("://")
            .is_some_and(|(_, rest)| !rest.is_empty() && !rest.starts_with('/'))
        && !target
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
}

pub fn parse_content(source: &str, _: SourceFormat) -> Result<ContentDocument, ProjectionError> {
    if source.is_empty() {
        return Err(ProjectionError::EmptySource);
    }
    if source.len() > MAXIMUM_SOURCE_BYTES {
        return Err(ProjectionError::SourceTooLarge {
            bytes: source.len(),
            maximum: MAXIMUM_SOURCE_BYTES,
        });
    }
    let mut roots = Vec::new();
    let mut count = 0;
    let mut stack: Vec<ContentNode> = Vec::new();
    for (event, range) in Parser::new_ext(source, Options::all()).into_offset_iter() {
        count += 1;
        if count > 4096 || stack.len() > 32 {
            return Err(ProjectionError::ContentLimit);
        }
        let mut node = ContentNode {
            kind: ContentKind::Text,
            start: range.start,
            end: range.end,
            text: String::new(),
            children: Vec::new(),
            level: None,
            ordinal: None,
            language: None,
            target: None,
            checked: None,
        };
        let is_start = matches!(&event, Event::Start(_));
        match event {
            Event::Start(tag) => {
                node.kind = match tag {
                    Tag::Paragraph => ContentKind::Paragraph,
                    Tag::Heading { level, .. } => {
                        node.level = Some(level as u8);
                        ContentKind::Heading
                    }
                    Tag::BlockQuote(_) => ContentKind::Quote,
                    Tag::CodeBlock(kind) => {
                        if let CodeBlockKind::Fenced(info) = kind {
                            node.language = info.split_whitespace().next().map(str::to_owned);
                        }
                        ContentKind::CodeBlock
                    }
                    Tag::HtmlBlock => ContentKind::HtmlBlock,
                    Tag::List(start) => {
                        node.ordinal = start;
                        ContentKind::List
                    }
                    Tag::Item => ContentKind::Item,
                    Tag::Table(_) => ContentKind::Table,
                    Tag::TableHead => ContentKind::TableHead,
                    Tag::TableRow => ContentKind::TableRow,
                    Tag::TableCell => ContentKind::TableCell,
                    Tag::Emphasis => ContentKind::Emphasis,
                    Tag::Strong => ContentKind::Strong,
                    Tag::Strikethrough => ContentKind::Strike,
                    Tag::Link { dest_url, .. } => {
                        node.target = Some(dest_url.into_string());
                        ContentKind::Link
                    }
                    Tag::Image { dest_url, .. } => {
                        node.target = Some(dest_url.into_string());
                        ContentKind::Image
                    }
                    Tag::FootnoteDefinition(label) => {
                        node.text = label.into_string();
                        ContentKind::Footnote
                    }
                    Tag::DefinitionList => ContentKind::DefinitionList,
                    Tag::DefinitionListTitle => ContentKind::DefinitionTitle,
                    Tag::DefinitionListDefinition => ContentKind::Definition,
                    Tag::Superscript => ContentKind::Superscript,
                    Tag::Subscript => ContentKind::Subscript,
                    Tag::MetadataBlock(_) => ContentKind::Metadata,
                };
            }
            Event::End(_) => {
                let mut completed = stack.pop().expect("pulldown-cmark balances events");
                completed.end = completed.end.max(range.end);
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(completed);
                } else {
                    roots.push(completed);
                }
                continue;
            }
            Event::Text(text) => {
                node.text = text.into_string();
            }
            Event::Code(text) => {
                node.kind = ContentKind::Code;
                node.text = text.into_string();
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                node.kind = ContentKind::Math;
                node.text = text.into_string();
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                node.kind = ContentKind::Html;
                node.text = text.into_string();
            }
            Event::FootnoteReference(text) => {
                node.kind = ContentKind::FootnoteReference;
                node.text = text.into_string();
            }
            Event::SoftBreak => node.kind = ContentKind::SoftBreak,
            Event::HardBreak => node.kind = ContentKind::HardBreak,
            Event::Rule => node.kind = ContentKind::Rule,
            Event::TaskListMarker(checked) => {
                node.kind = ContentKind::Task;
                node.checked = Some(checked);
            }
        }
        if is_start {
            stack.push(node);
        } else if let Some(parent) = stack.last_mut() {
            parent.children.push(node);
        } else {
            roots.push(node);
        }
    }
    Ok(ContentDocument {
        version: 1,
        nodes: roots,
    })
}
