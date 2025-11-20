use std::env::vars_os;

#[test]
fn hello() {
    dbg!(env!("CARGO_BIN_EXE_vite"));
}
