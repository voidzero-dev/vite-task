use std::io::{IsTerminal, stdin};

use subprocess_test::command_for_fn;

#[test]
fn is_terminal() {
    command_for_fn!((), |_: ()| { println!("{}", stdin().is_terminal()) });
}
