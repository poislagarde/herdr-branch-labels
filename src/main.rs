fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: herdr-branch-labels refresh\n\nPublish display-only sidebar labels.");
        return;
    }
    if args != ["refresh"] {
        eprintln!("Usage: herdr-branch-labels refresh");
        std::process::exit(2);
    }
    match herdr_branch_labels::runtime::refresh() {
        Ok(updated) => println!("{}", serde_json::json!({"updated": updated})),
        Err(error) => {
            eprintln!("branch-labels: {error}");
            std::process::exit(1);
        }
    }
}
