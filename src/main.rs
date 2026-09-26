fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = herdr_estate::cli::run(&argv);
    std::process::exit(code);
}
