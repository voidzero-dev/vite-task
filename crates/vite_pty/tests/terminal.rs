use std::{
    io::{IsTerminal, Write, stderr, stdin, stdout},
    thread,
    time::Duration,
};

use ntest::timeout;
use portable_pty::CommandBuilder;
use subprocess_test::command_for_fn;
use vite_pty::{geo::ScreenSize, terminal::Terminal};

#[test]
#[timeout(5000)]
fn is_terminal() {
    let cmd = CommandBuilder::from(command_for_fn!((), |_: ()| {
        println!("{} {} {}", stdin().is_terminal(), stdout().is_terminal(), stderr().is_terminal())
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    assert_eq!(output.trim(), "true true true");
}

#[test]
#[timeout(5000)]
fn read_until_single() {
    let cmd = CommandBuilder::from(command_for_fn!((), |_: ()| {
        println!("hello world");
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    terminal.read_until("hello").unwrap();
    terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    // After reading until "hello", the buffer should contain " world"
    // read_to_end should process the buffered data and continue reading
    assert!(output.contains("world"));
}

#[test]
#[timeout(5000)]
fn read_until_multiple_sequential() {
    let cmd = CommandBuilder::from(command_for_fn!((), |_: ()| {
        thread::sleep(Duration::from_millis(10));
        print!("first second third");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    terminal.read_until("first").unwrap();
    terminal.read_until("second").unwrap();
    terminal.read_until("third").unwrap();
    terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    // All three words should be in the screen
    assert!(output.contains("first"));
    assert!(output.contains("second"));
    assert!(output.contains("third"));
}

#[test]
#[timeout(5000)]
fn read_until_not_found() {
    let cmd = CommandBuilder::from(command_for_fn!((), |_: ()| {
        thread::sleep(Duration::from_millis(10));
        print!("hello world");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    let result = terminal.read_until("nonexistent");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Expected string not found"));
}

#[test]
#[timeout(5000)]
fn read_until_with_read_to_end() {
    let cmd = CommandBuilder::from(command_for_fn!((), |_: ()| {
        thread::sleep(Duration::from_millis(10));
        print!("prefix middle suffix");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    terminal.read_until("middle").unwrap();
    // At this point, " suffix" should be buffered
    terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    // The full output should include everything
    assert!(output.contains("prefix"));
    assert!(output.contains("middle"));
    assert!(output.contains("suffix"));
}

#[test]
#[timeout(5000)]
fn read_until_boundary_spanning() {
    // Test case where expected string might span across read boundaries
    let cmd = CommandBuilder::from(command_for_fn!((), |_: ()| {
        // Write in small chunks to increase chance of boundary spanning
        print!("a");
        let _ = stdout().flush();
        thread::sleep(Duration::from_millis(5));
        print!("b");
        let _ = stdout().flush();
        thread::sleep(Duration::from_millis(5));
        print!("c");
        let _ = stdout().flush();
        thread::sleep(Duration::from_millis(5));
        print!("d");
        let _ = stdout().flush();
        thread::sleep(Duration::from_millis(5));
        print!("e");
        let _ = stdout().flush();
        thread::sleep(Duration::from_millis(5));
        print!("f");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    // Search for a pattern that's likely to span boundaries
    terminal.read_until("abcd").unwrap();
    terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    assert!(output.contains("abcdef"));
}

#[test]
#[timeout(5000)]
fn read_until_exact_boundary() {
    // Test where we search for something at the exact boundary
    let cmd = CommandBuilder::from(command_for_fn!((), |_: ()| {
        print!("first");
        let _ = stdout().flush();
        thread::sleep(Duration::from_millis(10));
        print!("second");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    // This should find "second" even if "first" was in a previous read
    terminal.read_until("second").unwrap();
    terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    assert!(output.contains("first"));
    assert!(output.contains("second"));
}

#[test]
#[timeout(5000)]
fn read_until_after_read_to_end() {
    // Test that read_until works with data that comes after EOF
    let cmd = CommandBuilder::from(command_for_fn!((), |_: ()| {
        println!("hello world foo bar");
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    // Use read_until first to consume part of the data
    terminal.read_until("world").unwrap();

    // Read everything else
    terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    assert!(output.contains("hello world foo bar"));

    // After read_to_end, buffer is empty and we're at EOF
    // Trying to find anything should fail
    let result = terminal.read_until("bar");
    assert!(result.is_err());
}
