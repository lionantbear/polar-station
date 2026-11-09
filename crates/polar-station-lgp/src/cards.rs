//! Bounded desktop composition of the existing shared Markdown runs.
//! Message content remains canonical; decorations never change its source.
use crate::text::{body, safe, Transcript};
use polar_edge_presentation::{push_run, BackgroundRole, TextRun, TextStyle};
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;


const CACHE_BUDGET: usize = polar_lgp_reader::PRESENTATION_CACHE_BYTES;
#[derive(Clone, Default)]
pub(crate) struct Card { pub runs: Vec<TextRun>, pub tool_row: Option<usize> }
#[derive(Clone)]
pub(crate) struct Cache {
    rows: std::collections::VecDeque<(String, usize, Card)>,
    bytes: usize,
    limit: usize,
}
impl Default for Cache {fn default()->Self{Self{rows:Default::default(),bytes:0,limit:CACHE_BUDGET}}}
impl Cache {
    pub(crate) fn set_limit(&mut self, limit:usize) {
        self.limit=limit;
        while self.bytes>limit {
            let Some((_,size,_))=self.rows.pop_front()else{break;};self.bytes=self.bytes.saturating_sub(size);
        }
    }
    pub(crate) fn clear(&mut self) {
        self.rows.clear();
        self.bytes = 0;
    }
    pub(crate) fn invalidate(&mut self, prefix: &str) {
        self.rows.retain(|(key, bytes, _)| {
            if key.starts_with(prefix) {
                self.bytes = self.bytes.saturating_sub(*bytes);
                false
            } else {
                true
            }
        });
    }
    fn get(&mut self, key: String, build: impl FnOnce() -> Card) -> Card {
        if let Some(index) = self.rows.iter().position(|(stored, _, _)| stored == &key) {
            let entry = self.rows.remove(index).expect("existing cache entry");
            let result = entry.2.clone();
            self.rows.push_back(entry);
            return result;
        }
        let card = build();
        let runs = &card.runs;
        let bytes = key.capacity() + runs.capacity() * std::mem::size_of::<TextRun>()
            + runs.iter().map(|run|run.text.capacity() + run.style.link.as_ref().map_or(0,String::capacity)).sum::<usize>();
        if bytes <= self.limit {
            while self.bytes + bytes > self.limit {
                let Some((_, size, _)) = self.rows.pop_front() else {
                    break;
                };
                self.bytes = self.bytes.saturating_sub(size);
            }
            self.bytes += bytes;
            self.rows.push_back((key, bytes, card.clone()));
        }
        card
    }
}
pub(super) fn cached_card(v:&Value, transcript:&Transcript, columns:usize, generation:bool, expanded:Option<bool>)->Card {
    let key=if generation {format!("{}generation:{}:{}:{expanded:?}",transcript.cache_namespace,v["generation_id"],columns)}
        else {format!("{}{}",transcript.cache_namespace,serde_json::to_string(&("message",&v["sender_node_id_hex"],&v["message_id"],columns,expanded)).unwrap_or_default())};
    transcript.cards.borrow_mut().get(key,|| {
        let mut tool_row=None;
        let runs=if generation {generation_card_with_tools(v,columns,expanded,&mut tool_row)}
            else {message_card_with_tools(v,transcript,columns,expanded,&mut tool_row)};
        Card{runs,tool_row}
    })
}






fn author(v: &Value) -> (String, polar_lgp_presentation::AuthorPresentation) {
    let (label, identity) = polar_lgp_presentation::message_view_author(v);
    (single_line(&label), identity)
}

fn single_line(text: &str) -> String {
    safe(text).split_whitespace().collect::<Vec<_>>().join(" ")
}
fn clipped(text: &str, columns: usize) -> String {
    let mut out = String::new();
    let mut width = 0;
    for grapheme in text.graphemes(true) {
        if width + grapheme.width() > columns {
            break;
        }
        out.push_str(grapheme);
        width += grapheme.width();
    }
    out
}
fn heading(
    out: &mut Vec<TextRun>,
    name: &str,
    badge: &str,
    role: &str,
    clock: &str,
    color: Option<u8>,
    columns: usize,
) {
    let style = TextStyle {
        background: Some(BackgroundRole::MessageHeader),
        ..TextStyle::default()
    };
    let accent = TextStyle {
        bold: true,
        foreground: color,
        ..style.clone()
    };
    push_run(out, "▎ ", &accent);
    let clock = if columns >= 48 { clock } else { "" };
    let role = if columns >= 32 && !role.is_empty() {
        format!(" · {role}")
    } else {
        String::new()
    };
    let label = clipped(
        &format!("[{}] {name}", single_line(badge)),
        columns.saturating_sub(role.width() + clock.width() + 5),
    );
    push_run(out, &label, &accent);
    push_run(
        out,
        &role,
        &TextStyle {
            dim: true,
            ..style.clone()
        },
    );
    let padding = columns.saturating_sub(2 + label.width() + role.width() + clock.width());
    push_run(out, &" ".repeat(padding), &style);
    push_run(
        out,
        clock,
        &TextStyle {
            dim: true,
            ..style.clone()
        },
    );
    push_run(out, "\n", &style);
}
fn content_rows(
    out: &mut Vec<TextRun>,
    runs: Vec<TextRun>,
    columns: usize,
    color: Option<u8>,
    background: BackgroundRole,
) {
    let normal = TextStyle {
        background: Some(background),
        ..TextStyle::default()
    };
    let rail = TextStyle {
        foreground: color,
        ..normal.clone()
    };
    let mut column = 0;
    let mut started = false;
    let mut positions = std::collections::BTreeMap::new();
    for run in runs {
        let mut style = TextStyle {
            background: Some(background),
            ..run.style
        };
        let mut position = style.source.map_or(0, |source| *positions.get(&(source.start, source.end)).unwrap_or(&0));
        style.source_grapheme = position;
        for grapheme in safe(&run.text).graphemes(true) {
            if !started {
                push_run(out, "▎ ", &rail);
                started = true;
            }
            if grapheme == "\n" {
                push_run(
                    out,
                    &" ".repeat(columns.saturating_sub(2 + column)),
                    &normal,
                );
                push_run(out, "\n", &normal);
                column = 0;
                started = false;
                position += 1;
                style.source_grapheme = position;
                continue;
            }
            let mut width = if grapheme == "\t" {
                4 - column % 4
            } else {
                grapheme.width()
            };
            if column > 0 && column + width > columns - 4 {
                push_run(
                    out,
                    &" ".repeat(columns.saturating_sub(2 + column)),
                    &normal,
                );
                push_run(out, "\n", &normal);
                push_run(out, "▎ ", &rail);
                column = 0;
                style.source_grapheme = position;
                if grapheme == "\t" {
                    width = 4;
                }
            }
            if grapheme == "\t" {
                push_run(out, &" ".repeat(width), &style);
            } else {
                push_run(out, grapheme, &style);
            }
            column += width;
            position += 1;
        }
        if let Some(source) = style.source { positions.insert((source.start,source.end),position); }
    }
    if started {
        push_run(
            out,
            &" ".repeat(columns.saturating_sub(2 + column)),
            &normal,
        );
        push_run(out, "\n", &normal);
    }
}
fn content(v: &Value, columns: usize) -> Vec<TextRun> {
    if crate::state::inquiry::offer(v).is_some() || v["payload"]["value"]["message_kind"]=="inquiry_graph.session.result" {
        return vec![TextRun{text:crate::text::body(v),style:TextStyle::default()}];
    }
    let source = body(v);
    let (doc, fallback)=serde_json::from_value(v["presentation"]["content"].clone())
        .ok().map(|doc|(Some(doc),None))
        .unwrap_or_else(||polar_lgp_presentation::message_content(&source));
    let mut runs=if let Some(doc)=doc {
        polar_edge_presentation::screen_runs_for_width(&doc,columns.saturating_sub(4))
    } else if let Some(reason)=fallback {
        vec![TextRun{text:format!("[Source view — {}]\n",polar_lgp_presentation::content_fallback_label(&reason)),style:TextStyle::default()},TextRun{text:source.clone(),style:TextStyle{source:Some(polar_edge_presentation::SourceSpan{start:0,end:source.len()}),..Default::default()}}]
    } else {Vec::new()};
    while let Some(last) = runs.last_mut() {
        if last.style.code {
            break;
        }
        last.text.truncate(last.text.trim_end_matches('\n').len());
        if last.text.is_empty() {
            runs.pop();
        } else {
            break;
        }
    }
    runs
}
// Derive a task view from the existing canonical offer and its causal children.
// Every checkpoint keeps a physical row and message identity for reader anchors.
fn routing_card(v:&Value, transcript:&Transcript) -> Option<(String,String,bool)> {
    let kind=v["payload"]["value"]["message_kind"].as_str()?;
    let compact=matches!(kind,"inquiry_graph.session.result"|"station.session.delivery.receipt");
    let offer=if compact {
        transcript.messages.get(&(v["causal_sender_id"].as_str()?.into(),v["causal_message_id"].as_str()?.into()))?
    } else {v};
    let (payload,_)=crate::state::inquiry::offer(offer)?;
    let package=&payload["package"];
    let metadata=polar_station_session::routing::form_metadata(&package["metadata"])?;
    let children=transcript.messages.values().filter(|row|row["causal_sender_id"]==offer["sender_node_id_hex"]
        && row["causal_message_id"]==offer["message_id"] && row["thread_id"]==offer["thread_id"]);
    let result=children.clone().filter(|row| row["payload"]["value"]["message_kind"]=="inquiry_graph.session.result"
        && matches!(row["author_kind"].as_str(),Some("local_human"|"system_runtime")))
        .max_by_key(|row|(row["created_at"].as_u64().unwrap_or(0),row["message_id"].as_str().unwrap_or("")))
        .map(|row|&row["payload"]["value"]["result"]);
    let answers=result.and_then(|result|result["answers"].as_array());
    let selected=answers.and_then(|answers|answers.iter().find(|a|a["node_id"]=="target_session"))
        .and_then(|a|a["choice_ids"][0].as_str());
    let text=answers.and_then(|answers|answers.iter().find(|a|a["node_id"]=="task_text"))
        .and_then(|a|a["text"].as_str()).unwrap_or_else(||metadata["draft_text"].as_str().unwrap_or(""));
    let choices=package["nodes"].as_array()?.iter().find(|n|n["node_id"]=="target_session")?["choices"].as_array()?;
    let choice=selected.and_then(|id|choices.iter().find(|c|c["choice_id"]==id));
    let receipt=children.filter(|row|row["author_kind"]=="system_runtime"
        && row["payload"]["value"]["message_kind"]=="station.session.delivery.receipt"
        && result.is_some_and(|result|row["payload"]["value"]["inquiry_session_id"]==result["session_id"]))
        .max_by_key(|row|(row["created_at"].as_u64().unwrap_or(0),row["message_id"].as_str().unwrap_or("")))
        .map(|row|&row["payload"]["value"]["receipt"]);
    let status=match receipt.and_then(|receipt|receipt["state"].as_str()) {
        Some("delivered")=>"Terminal input delivered",
        Some("failed_before_write")=>"Failed before terminal input",
        Some(_)=>"Terminal delivery unknown — check before retrying",
        None if result.is_some()=>"Confirmed — waiting for terminal receipt",
        None=>"Task awaiting confirmation",
    };
    let target=|choice:&Value|format!("{} · {} · {}",choice["label"].as_str().unwrap_or("Session"),
        choice["metadata"]["agent_type"].as_str().unwrap_or("terminal"),choice["metadata"]["host"].as_str().unwrap_or("Host"));
    if compact {
        let label=if kind=="inquiry_graph.session.result" {"Task confirmed"} else {status};
        return Some((label.into(),choice.map(target).unwrap_or_default(),true));
    }
    let mut body=text.to_owned();
    for c in choice.into_iter().chain(if selected.is_none(){choices.iter().collect::<Vec<_>>()}else{vec![]}) {
        body.push_str(&format!("\n{}\n{}",target(c),c["metadata"]["directory"].as_str().unwrap_or("")));
        if let Some(summary)=c["metadata"]["summary"].as_array() {
            for line in summary.iter().filter_map(Value::as_str) {body.push('\n');body.push_str(line);}
        }
        if selected.is_none() {if let Some(reason)=c["metadata"]["reason"].as_str(){body.push('\n');body.push_str(reason);}}
    }
    if selected.is_none(){body.push_str("\nChoose a target in the Chat Bar.");}
    Some((status.into(),body,false))
}
fn message_card_with_tools(v: &Value, transcript: &Transcript, columns: usize, expanded:Option<bool>, tool_row:&mut Option<usize>) -> Vec<TextRun> {
    let (name, identity) = author(v);
    let clock = v["created_at"]
        .as_u64()
        .and_then(|s| i64::try_from(s).ok())
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    if let Some((status,body,compact))=routing_card(v,transcript) {
        if compact {
            let text=clipped(&format!("• {status} · {}",single_line(&body)),columns.saturating_sub(4));
            content_rows(&mut out,vec![TextRun{text,style:TextStyle{dim:true,..Default::default()}}],columns,identity.color,BackgroundRole::Reference);
        } else {
            heading(&mut out,&status,"TASK","",&clock,identity.color,columns);
            content_rows(&mut out,vec![TextRun{text:body,style:TextStyle::default()}],columns,identity.color,BackgroundRole::MessageBody);
        }
        return out;
    }
    heading(
        &mut out,
        &name,
        &identity.badge,
        &identity.role,
        &clock,
        identity.color,
        columns,
    );
    let causal = &v["presentation"]["causal"];
    if let (Some(sender), Some(message)) = (
        causal["key"]["sender_id"].as_str().or_else(|| v["causal_sender_id"].as_str()),
        causal["key"]["message_id"].as_str().or_else(|| v["causal_message_id"].as_str()),
    ) {
        let label = causal["label"].as_str().unwrap_or_else(||
            polar_lgp_presentation::causal_label(v["causal_kind"].as_str()));
        let reference = transcript
            .messages
            .get(&(sender.to_owned(), message.to_owned()));
        let summary = reference
            .map(|original| {
                format!(
                    "{} · {}",
                    author(original).0,
                    body(original)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })
            .unwrap_or_else(|| "Original message not loaded".into());
        let text = clipped(&format!("↳ {label} {summary}"), columns - 4);
        content_rows(
            &mut out,
            vec![TextRun {
                text,
                style: TextStyle {
                    dim: true,
                    ..TextStyle::default()
                },
            }],
            columns,
            identity.color,
            BackgroundRole::Reference,
        );
    }
    content_rows(
        &mut out,
        content(v, columns),
        columns,
        identity.color,
        BackgroundRole::MessageBody,
    );
    for attachment in crate::text::attachments(v) {
        let text = format!(
            "[Attachment] {}",
            safe(attachment["name"].as_str().unwrap_or("file"))
        );
        content_rows(
            &mut out,
            vec![TextRun {
                text,
                style: TextStyle::default(),
            }],
            columns,
            identity.color,
            BackgroundRole::MessageBody,
        );
    }
    if let Some(activity)=transcript.message_activity(v) {
        append_tools(&mut out,activity,expanded.unwrap_or(false),columns,identity.color,tool_row);
    } else if polar_lgp_reader::generation_id(v).is_some() {
        append_tools(&mut out,&serde_json::json!({"tools":[]}),expanded.unwrap_or(false),columns,identity.color,tool_row);
    }
    out
}
fn append_tools(out:&mut Vec<TextRun>, activity:&Value, expanded:bool, columns:usize, color:Option<u8>, tool_row:&mut Option<usize>) {
    let Some(tools)=activity["tools"].as_array() else{return;};
    *tool_row=Some(out.iter().map(|run|run.text.matches('\n').count()).sum());
    let count=if tools.is_empty() && activity["history_loaded"]!=true {String::new()} else {format!(" ({})",tools.len())};
    let mut rows=vec![format!("{} Tool activity{count}",if expanded{"▾"}else{"▸"})];
    if expanded {
        if tools.is_empty() {rows.push(if activity["history_loaded"]==true {"No recorded tool calls are available."}else{"Loading recorded tool calls…"}.into());}
        rows.extend(tools.iter().enumerate().map(|(index,tool)| {
            let label=serde_json::from_value::<polar_inhouse_model::ReplyToolState>(tool["state"].clone())
                .map(polar_lgp_presentation::tool_state_label).unwrap_or("Unknown");
            format!("{}. {} · {label}",index+1,safe(tool["tool_name"].as_str().unwrap_or("Tool")))
        }));
    }
    content_rows(out,vec![TextRun{text:rows.join("\n"),style:TextStyle{dim:true,..Default::default()}}],columns,color,BackgroundRole::MessageBody);
}

fn generation_card_with_tools(g: &Value, columns: usize, expanded:Option<bool>, tool_row:&mut Option<usize>) -> Vec<TextRun> {
    let name = g["actor_name"].as_str().unwrap_or("Agent");
    let identity = polar_lgp_presentation::author_presentation(
        "inhouse_polar",
        g["owner"].as_str().unwrap_or(""),
        g["actor_id"].as_str(),
        name,
    );
    let presentation = crate::text::generation_presentation(g);
    let mut out = Vec::new();
    heading(
        &mut out,
        &single_line(name),
        &identity.badge,
        "Polar",
        &single_line(&presentation.label),
        identity.color,
        columns,
    );
    let answer = serde_json::json!({"payload":{"text":g["answer"].as_str().unwrap_or("")}});
    content_rows(
        &mut out,
        content(&answer, columns),
        columns,
        identity.color,
        BackgroundRole::MessageBody,
    );
    let tools=g["tools"].as_array().filter(|tools|!tools.is_empty());
    if tools.is_some() {
        append_tools(&mut out,g,expanded.unwrap_or_else(||!polar_lgp_reader::tools_collapsed(g)),columns,identity.color,tool_row);
    }
    if (tools.is_none() || presentation.kind != polar_lgp_presentation::ActivityKind::Tool) && !presentation.status_text.is_empty() {
        content_rows(
            &mut out,
            vec![TextRun {
                text: safe(&presentation.status_text),
                style: TextStyle {
                    dim: true,
                    ..TextStyle::default()
                },
            }],
            columns,
            identity.color,
            BackgroundRole::MessageBody,
        );
    }
    out
}




