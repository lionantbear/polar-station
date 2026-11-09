use serde_json::Value;
const MAX_RECORD:usize=4*1024*1024;
#[derive(Default)]
pub(crate) struct Assembler { active:Option<(u64,usize,String)> }
impl Assembler {
    pub fn clear(&mut self){self.active=None;}
    pub fn accept(&mut self,p:&Value)->Result<Option<Value>,String>{
        let invalid=||"invalid or noncontiguous event record".to_owned();
        match p["kind"].as_str().ok_or_else(invalid)? {
            "record_begin"=>{
                let id=p["record"].as_u64().ok_or_else(invalid)?;
                let size=p["bytes"].as_u64().ok_or_else(invalid)?;
                if self.active.is_some()||id==0||size==0||size>MAX_RECORD as u64{return Err(invalid());}
                self.active=Some((id,size as usize,String::new()));Ok(None)
            },
            "record_chunk"=>{
                let (id,size,text)=self.active.as_mut().ok_or_else(invalid)?;
                let chunk=p["text"].as_str().ok_or_else(invalid)?;
                if p["record"].as_u64()!=Some(*id)||p["offset"].as_u64()!=Some(text.len() as u64)||
                    chunk.is_empty()||chunk.len()>4096||text.len().saturating_add(chunk.len())>*size{return Err(invalid());}
                text.push_str(chunk);Ok(None)
            },
            "record_end"=>{
                let (id,size,text)=self.active.take().ok_or_else(invalid)?;
                if p["record"].as_u64()!=Some(id)||text.len()!=size{return Err(invalid());}
                serde_json::from_str(&text).map(Some).map_err(|_|invalid())
            },
            _=>Err(invalid()),
        }
    }
}

