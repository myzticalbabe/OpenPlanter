//! op-fast CLI: `list | search | fp | wiki-scan | patch-check`.
//! Stdout is machine-readable (paths, rg-style hits, JSON lines).

use std::path::PathBuf;

fn usage() -> &'static str {
    "op-fast <list|search|fp|wiki-scan|patch-check> ...\n\
     list <root> [--glob PAT]\n\
     search <root> <query> [--glob PAT] [--max N]\n\
     fp <root>\n\
     wiki-scan <dir>\n\
     patch-check <file>"
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
    }
    None
}

fn main() {
    if let Err(e) = run() {
        eprintln!("op-fast: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    match cmd {
        "list" => {
            let root = PathBuf::from(args.get(1).map(|s| s.as_str()).unwrap_or("."));
            let glob = flag(&args, "--glob");
            for rel in op_fast::list_files(&root, glob.as_deref()) {
                println!("{rel}");
            }
        }
        "search" => {
            let root = PathBuf::from(args.get(1).map(|s| s.as_str()).unwrap_or("."));
            let query = args.get(2).cloned().unwrap_or_default();
            let glob = flag(&args, "--glob");
            let max: usize = flag(&args, "--max")
                .and_then(|v| v.parse().ok())
                .unwrap_or(200);
            for hit in op_fast::search_files(&root, &query, glob.as_deref(), max) {
                println!("{hit}");
            }
        }
        "fp" => {
            let root = PathBuf::from(args.get(1).map(|s| s.as_str()).unwrap_or("."));
            for fp in op_fast::fingerprint_dir(&root) {
                println!("{}", serde_json::to_string(&fp)?);
            }
        }
        "wiki-scan" => {
            let dir = PathBuf::from(args.get(1).map(|s| s.as_str()).unwrap_or("."));
            for row in op_fast::scan_wiki_index(&dir) {
                println!("{}", serde_json::to_string(&row)?);
            }
        }
        "patch-check" => {
            let file = args.get(1).cloned().unwrap_or_default();
            let text = std::fs::read_to_string(&file)?;
            match op_fast::check_patch(&text) {
                Ok(n) => println!("OK {n} ops"),
                Err(e) => {
                    eprintln!("INVALID: {e}");
                    std::process::exit(2);
                }
            }
        }
        _ => {
            eprintln!("{usage}", usage = usage());
            std::process::exit(2);
        }
    }
    Ok(())
}
