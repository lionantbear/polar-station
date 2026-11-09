//! Development/test executable only. The product uses the in-process extension.
use std::io::{self, BufRead};
fn main() {
    if std::env::args().nth(1).as_deref()==Some("--benchmark") {
        for (count,steps) in [(100,300),(1000,300),(4000,300)] {
            let nodes:Vec<_>=(0..count).map(|i|serde_json::json!({"id":format!("n{i}")})).collect();
            let links:Vec<_>=(1..count).map(|i|serde_json::json!({"source":format!("n{}",(i-1)/3),"target":format!("n{i}")})).collect();
            let input=serde_json::json!({"nodes":nodes,"links":links}).to_string();
            let start=std::time::Instant::now();
            let output=polar_force_layout_core::layout_json(&input,steps,None);
            let elapsed=start.elapsed().as_secs_f64()*1000.0;
            let output:serde_json::Value=serde_json::from_str(&output).unwrap();
            if output["ok"]!=true { eprintln!("{output}");std::process::exit(1); }
            println!("{}",serde_json::json!({"nodes":count,"links":count-1,"steps":steps,"milliseconds":elapsed,"ok":true}));
        }
        return;
    }
    let input: Box<dyn BufRead> = if let Some(path)=std::env::args().nth(1) {
        match std::fs::File::open(&path) {
            Ok(file)=>Box::new(io::BufReader::new(file)),
            Err(error)=>{ eprintln!("cannot read {path}: {error}"); std::process::exit(2); }
        }
    } else { Box::new(io::BufReader::new(io::stdin())) };
    for line in input.lines() {
        let output = match line {
            Ok(line) => match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(value) => {
                    let steps = value.get("steps").and_then(|s|s.as_u64()).unwrap_or(300) as usize;
                    let graph=value.get("graph").unwrap_or(&value).to_string();
                    polar_force_layout_core::layout_json(&graph,steps,None)
                },
                Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}).to_string(),
            },
            Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}).to_string(),
        };
        println!("{output}");
    }
}
