//! engram-eval - run a deterministic, offline regression suite of agent-harness replay
//! cases. `engram-eval` runs the built-in suite; `engram-eval <dir>` runs every `*.json`
//! case in a directory. Exit 0 = all pass, 1 = a regression, 2 = setup error.

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cases = match args.get(1) {
        Some(dir) => match engram_eval::load_dir(std::path::Path::new(dir)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("load error: {e}");
                std::process::exit(2);
            }
        },
        None => engram_eval::builtin_cases(),
    };
    if cases.is_empty() {
        eprintln!("no eval cases found");
        std::process::exit(2);
    }

    let (mut passed, mut failed) = (0u32, 0u32);
    for case in &cases {
        match engram_eval::run_case(case).await {
            Ok(out) => {
                let fails = engram_eval::check(case, &out);
                if fails.is_empty() {
                    println!("PASS  {}", case.name);
                    passed += 1;
                } else {
                    println!("FAIL  {}", case.name);
                    for f in &fails {
                        println!("        · {f}");
                    }
                    failed += 1;
                }
            }
            Err(e) => {
                println!("ERROR {} - {e}", case.name);
                failed += 1;
            }
        }
    }
    println!("\n{passed} passed, {failed} failed, {} total", cases.len());
    if failed > 0 {
        std::process::exit(1);
    }
}
