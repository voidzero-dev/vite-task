use std::io::{IsTerminal, stderr, stdin, stdout};

use portable_pty::CommandBuilder;
use subprocess_test::command_for_fn;
use vite_pty::{geo::ScreenSize, terminal::Terminal};

#[test]
fn is_terminal() {
    let cmd = CommandBuilder::from(command_for_fn!((), |_: ()| {
        println!("{} {} {}", stdin().is_terminal(), stdout().is_terminal(), stderr().is_terminal())
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    let output = terminal.read_to_end().unwrap();
    assert_eq!(output.trim(), "true true true");
}
