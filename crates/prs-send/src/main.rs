fn main() {
    if let Err(error) = prs_send::run(
        std::env::args().skip(1),
        std::io::stdout(),
        std::io::stderr(),
    ) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
