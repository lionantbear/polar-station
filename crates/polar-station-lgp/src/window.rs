//! Bounded Ghostty windows positioned by immutable content, not scroll offsets.
use crate::{cards, text::Transcript};
use polar_edge_presentation::{push_run, TextRun};
use polar_lgp_reader::SourceAnchor;
use serde_json::Value;
use std::collections::VecDeque;
use unicode_segmentation::UnicodeSegmentation;

const BUDGET: usize = 60 * 1024;
#[derive(Clone, Default)]
pub(crate) struct Window {
    pub runs: Vec<TextRun>,
    pub anchors: Vec<SourceAnchor>,
    pub references: Vec<Reference>,
    pub more_before: bool,
    pub more_after: bool,
    pub restore_row: Option<usize>,
    pub gap_before: Option<(String,String)>,
    pub gap_after: Option<(String,String)>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Reference {
    pub row: usize,
    pub column_end: usize,
    pub target_row: Option<usize>,
    pub label: String,
    pub kind: &'static str,
    #[serde(skip)]
    pub generation: Option<String>,
    #[serde(skip)]
    pub target: (String,String),
}
struct Row { runs: Vec<TextRun>, anchor: SourceAnchor, bytes: usize, tool:Option<String> }
struct Item<'a> { value:&'a Value, generation:bool, key:(String,String) }

pub(crate) fn highlight(window:&mut Window,key:&(String,String)) {
    let Some(target)=window.anchors.iter().position(|anchor|&anchor.key==key&&anchor.end==0)else{return;};
    let mut line=0;
    for run in &mut window.runs {
        if line==target && run.text.starts_with("▎ "){run.text=run.text.replacen("▎ ","▶ ",1);return;}
        line+=run.text.matches('\n').count();
    }
}

fn rows(item:&Item<'_>, transcript:&Transcript, columns:usize, tools:&std::collections::BTreeMap<String,Disclosure>)->Vec<Row> {
    let activity=if item.generation {Some(item.value)} else {transcript.message_activity(item.value)};
    let generation=activity.and_then(|v|v["generation_id"].as_str()).or_else(||polar_lgp_reader::generation_id(item.value));
    let card=cards::cached_card(item.value,transcript,columns,item.generation,generation.and_then(|id|tools.get(id)).map(|state|state.open));
    let tool_row=card.tool_row;
    let runs=card.runs;
    let mut output=Vec::new();let mut current=Vec::new();let mut anchor=None;
    let fallback=SourceAnchor{key:item.key.clone(),start:0,end:0,grapheme:0};
    for run in runs {
        let mut offset=run.style.source_grapheme;
        for part in run.text.split_inclusive('\n') {
            if let Some(source)=run.style.source {
                if anchor.is_none() {anchor=Some(SourceAnchor{key:item.key.clone(),start:source.start,end:source.end,grapheme:offset});}
                offset+=part.graphemes(true).count();
            }
            push_run(&mut current,part,&run.style);
            if part.ends_with('\n') {
                let runs=std::mem::take(&mut current);
                let bytes=serde_json::to_vec(&runs).map_or(BUDGET,|bytes|bytes.len()+1);
                output.push(Row{tool:if tool_row==Some(output.len()){generation.map(str::to_owned)}else{None},runs,anchor:anchor.take().unwrap_or_else(||fallback.clone()),bytes});
            }
        }
    }
    if !current.is_empty() {
        let bytes=serde_json::to_vec(&current).map_or(BUDGET,|bytes|bytes.len()+1);
        output.push(Row{tool:None,runs:current,anchor:anchor.unwrap_or(fallback),bytes});
    }
    output
}

fn anchor_row(rows:&[Row],anchor:&SourceAnchor)->usize {
    rows.iter().enumerate().min_by_key(|(_,row)|(
        usize::from(row.anchor.start!=anchor.start || row.anchor.end!=anchor.end),
        row.anchor.start.abs_diff(anchor.start), row.anchor.grapheme.abs_diff(anchor.grapheme)))
        .map_or(0,|(index,_)|index)
}

#[derive(Clone,Copy)]
pub(crate) struct Disclosure {pub open:bool, terminal:bool}
pub(crate) fn reconcile_tools(transcript:&Transcript, states:&mut std::collections::BTreeMap<String,Disclosure>) {
    let mut present=std::collections::BTreeSet::new();
    for (value,finalized) in transcript.generations.values().map(|v|(v,false))
        .chain(transcript.messages.values().filter_map(|message|transcript.message_activity(message).map(|v|(v,true)))) {
        let Some(id)=value["generation_id"].as_str() else{continue;};
        present.insert(id.to_owned());
        let terminal=finalized||polar_lgp_reader::tools_collapsed(value);
        let state=states.entry(id.to_owned()).or_insert(Disclosure{open:!terminal,terminal});
        if terminal&&!state.terminal {state.open=false;state.terminal=true;}
    }
    for id in transcript.messages.values().filter_map(polar_lgp_reader::generation_id) {
        present.insert(id.to_owned());states.entry(id.to_owned()).or_insert(Disclosure{open:false,terminal:true});
    }
    states.retain(|id,_|present.contains(id));
}

pub(crate) fn render_with_tools(transcript:&Transcript,columns:usize,anchor:Option<&SourceAnchor>,tail:bool,tools:&std::collections::BTreeMap<String,Disclosure>)->Window {
    let mut values=transcript.messages.values().collect::<Vec<_>>();
    values.sort_by_key(|row|polar_lgp_reader::order(row));
    let mut items=values.into_iter().filter_map(|value|polar_lgp_reader::message_key(value).ok()
        .map(|key|Item{value,generation:false,key})).collect::<Vec<_>>();
    items.extend(transcript.generations.iter().map(|(id,value)|Item{value,generation:true,key:(String::new(),id.clone())}));
    if items.is_empty(){return Window::default();}
    let columns=columns.clamp(12,512);
    let index=anchor.and_then(|anchor|items.iter().position(|item|item.key==anchor.key))
        .unwrap_or(if tail {items.len()-1}else{0});
    let initial=rows(&items[index],transcript,columns,tools);
    if initial.is_empty(){return Window::default();}
    let center=anchor.map(|anchor|anchor_row(&initial,anchor))
        .unwrap_or(if tail{initial.len()-1}else{0});
    let mut selected=VecDeque::new();let mut used=2;
    let before_budget=if anchor.is_some(){BUDGET/2}else if tail{BUDGET}else{0};
    let mut more_before=false;let mut before_rows=0;let mut gap_before=None;let mut gap_after=None;
    for item_index in (0..=index).rev() {
        if item_index<index && !items[item_index].generation && !items[item_index+1].generation
            && !transcript.connected(&items[item_index].key,&items[item_index+1].key) {
            gap_before=Some(items[item_index+1].key.clone());break;
        }
        let card=rows(&items[item_index],transcript,columns,tools);
        let end=if item_index==index{center}else{card.len()};
        for row in card.into_iter().take(end).rev() {
            if used+row.bytes+initial[center].bytes>BUDGET || used+row.bytes>before_budget {
                more_before=true;break;
            }
            used+=row.bytes;before_rows+=1;selected.push_front(row);
        }
        if more_before{break;}
    }
    let mut more_after=false;
    for (item_index,item) in items.iter().enumerate().skip(index) {
        if item_index>index && !item.generation && !items[item_index-1].generation
            && !transcript.connected(&items[item_index-1].key,&item.key) {
            gap_after=Some(items[item_index-1].key.clone());break;
        }
        let card=rows(item,transcript,columns,tools);
        let start=if item_index==index{center}else{0};
        for row in card.into_iter().skip(start) {
            if used+row.bytes>BUDGET {more_after=true;break;}
            used+=row.bytes;selected.push_back(row);
        }
        if more_after{break;}
    }
    if !more_after && gap_after.is_none() {
        if let Some(last)=selected.back().filter(|row|!row.anchor.key.0.is_empty()) {
            if transcript.range_edges(&last.anchor.key).1==Some(true){gap_after=Some(last.anchor.key.clone());}
        }
    }
    let mut result=Window{more_before,more_after,gap_before,gap_after,restore_row:anchor.map(|_|before_rows),..Default::default()};
    let mut headers=std::collections::BTreeMap::new();
    for row in selected {
        if row.runs.iter().any(|run|run.style.background==Some(polar_edge_presentation::BackgroundRole::MessageHeader)) {
            headers.entry(row.anchor.key.clone()).or_insert(result.anchors.len());
        }
        if row.runs.iter().any(|run|run.style.background==Some(polar_edge_presentation::BackgroundRole::Reference)) {
            if let Some(message)=transcript.messages.get(&row.anchor.key) {
                let causal=&message["presentation"]["causal"];
                if let (Some(sender),Some(id))=(causal["key"]["sender_id"].as_str().or_else(||message["causal_sender_id"].as_str()),
                    causal["key"]["message_id"].as_str().or_else(||message["causal_message_id"].as_str())) {
                    result.references.push(Reference{row:result.anchors.len(),column_end:4,target_row:None,kind:"reference",generation:None,
                        label:row.runs.iter().map(|run|run.text.as_str()).collect::<String>().trim().trim_start_matches('▎').trim().to_owned(),
                        target:(sender.to_ascii_lowercase(),id.to_owned())});
                }
            }
        }
        if row.runs.iter().any(|run|run.style.background==Some(polar_edge_presentation::BackgroundRole::MessageBody)) && transcript.messages.get(&row.anchor.key).is_some_and(|m|crate::state::inquiry::offer(m).is_some()) {
            result.references.push(Reference{row:result.anchors.len(),column_end:columns,target_row:None,label:"Handle Inquiry".into(),kind:"inquiry",generation:None,target:row.anchor.key.clone()});
        }
        if let Some(generation)=row.tool {
            result.references.push(Reference{row:result.anchors.len(),column_end:unicode_width::UnicodeWidthStr::width(row.runs.iter().map(|run|run.text.as_str()).collect::<String>().trim_end()).min(columns),target_row:None,label:"Toggle tool activity".into(),
                kind:"tool",generation:Some(generation),target:row.anchor.key.clone()});
        }
        result.anchors.push(row.anchor);
        for run in row.runs {push_run(&mut result.runs,&run.text,&run.style);}
    }
    for reference in &mut result.references {
        reference.target_row=headers.get(&reference.target).copied();
    }
    result
}




