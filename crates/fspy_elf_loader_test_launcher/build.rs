fn main() {
    if let Ok(link_args) = std::env::var("DEP_FSPY_ELF_LOADER_LINK_ARGS") {
        for argument in link_args.split_whitespace() {
            println!("cargo::rustc-link-arg={argument}");
        }
    }
}
