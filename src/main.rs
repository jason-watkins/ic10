use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: ic20c <source.ic20>");
        process::exit(1);
    }
    let path = &args[1];
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read '{}': {}", path, e);
            process::exit(1);
        }
    };
    // Compiler pipeline will be wired in here as each phase is implemented.
    let _ = source;
    eprintln!("ic20c: compiler not yet implemented");
    process::exit(1);
}
