//! Only complete canonical page/context reads establish continuous history.
use serde_json::Value;
type Position = (u64, String, String);
#[derive(Clone)]
struct Range { begin:Position, end:Position, before:Option<bool>, after:Option<bool>, begin_excluded:bool, end_excluded:bool }
impl Range {
    fn contains(&self,position:&Position)->bool {
        (&self.begin<position || (!self.begin_excluded && &self.begin==position))
            && (position<&self.end || (!self.end_excluded && position==&self.end))
    }
}
#[derive(Clone, Default)]
pub(crate) struct Coverage(Vec<Range>);
fn position(row:&Value)->Position {
    let (time,message,sender)=crate::order(row);
    (time,message.to_owned(),sender.to_ascii_lowercase())
}
impl Coverage {
    pub fn bytes(&self)->usize {
        self.0.capacity()*std::mem::size_of::<Range>()+self.0.iter().map(|range|
            range.begin.1.capacity()+range.begin.2.capacity()+range.end.1.capacity()+range.end.2.capacity()).sum::<usize>()
    }
    pub fn include<'a>(&mut self,rows:impl Iterator<Item=&'a Value>,before:Option<bool>,after:Option<bool>) {
        let mut rows=rows.map(position);
        let Some(first)=rows.next()else{return;};
        let (mut begin,mut end)=(first.clone(),first);
        for row in rows {begin=begin.min(row.clone());end=end.max(row);}
        self.0.push(Range{begin,end,before,after,begin_excluded:false,end_excluded:false});self.normalize();
    }
    fn normalize(&mut self) {
        self.0.sort_by(|a,b|a.begin.cmp(&b.begin));
        let mut merged:Vec<Range>=Vec::new();
        for range in self.0.drain(..) {
            if let Some(last)=merged.last_mut().filter(|last|range.begin<last.end || (range.begin==last.end && !range.begin_excluded && !last.end_excluded)) {
                if range.begin==last.begin {last.begin_excluded &= range.begin_excluded;if range.before.is_some(){last.before=range.before;}}
                if range.end>last.end {last.end=range.end;last.after=range.after;last.end_excluded=range.end_excluded;}
                else if range.end==last.end {last.end_excluded &= range.end_excluded;if range.after.is_some(){last.after=range.after;}}
            }else{merged.push(range);}
        }
        self.0=merged;
    }
    pub fn contains(&self,a:&Value,b:&Value)->bool {
        let a=position(a);let b=position(b);
        self.0.iter().any(|range|range.contains(&a)&&range.contains(&b))
    }
    pub fn edges(&self,row:&Value)->(Option<bool>,Option<bool>) {
        let row=position(row);
        self.0.iter().find(|range|range.contains(&row))
            .map_or((None,None),|range|(range.before,range.after))
    }
    pub fn invalidate(&mut self,row:&Value) {
        let row=position(row);
        // Split proof at the removed position, retaining certainty inside the
        // still-cached sides. Exclusive edges must never join across the hole.
        let mut kept=Vec::new();
        for range in self.0.drain(..) {
            if !range.contains(&row) {kept.push(range);continue;}
            if range.begin<row {kept.push(Range{end:row.clone(),end_excluded:true,after:Some(true),..range.clone()});}
            if row<range.end {kept.push(Range{begin:row.clone(),begin_excluded:true,before:Some(true),..range});}
        }
        self.0=kept;
    }
    pub fn union(&mut self,other:Self) {self.0.extend(other.0);self.normalize();}
}
