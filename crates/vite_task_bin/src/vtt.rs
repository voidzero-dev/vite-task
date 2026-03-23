//! `vtt` — Vite Task Tools
//!
//! A lightweight test utility binary with subcommands used by e2e and plan snapshot tests.
//! Replaces the Node.js tools previously in `packages/tools`.

// This is a standalone test utility binary that deliberately uses std types
// rather than the project's custom types (vite_str, vite_path, etc.).
#![expect(clippy::disallowed_types, reason = "standalone test utility uses std types")]
#![expect(clippy::disallowed_macros, reason = "standalone test utility uses std macros")]
#![expect(clippy::disallowed_methods, reason = "standalone test utility uses std methods")]
#![expect(clippy::print_stderr, reason = "CLI tool error output")]
#![expect(clippy::print_stdout, reason = "CLI tool output")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: vtt <subcommand> [args...]");
        eprintln!(
            "Subcommands: check-tty, json-edit, print, print-env, print-file, read-stdin, replace-file-content, touch-file"
        );
        std::process::exit(1);
    }

    let result: Result<(), Box<dyn std::error::Error>> = match args[1].as_str() {
        "check-tty" => {
            cmd_check_tty();
            Ok(())
        }
        "json-edit" => cmd_json_edit(&args[2..]),
        "print" => {
            cmd_print(&args[2..]);
            Ok(())
        }
        "print-env" => cmd_print_env(&args[2..]),
        "print-file" => cmd_print_file(&args[2..]),
        "read-stdin" => cmd_read_stdin(),
        "replace-file-content" => cmd_replace_file_content(&args[2..]),
        "touch-file" => cmd_touch_file(&args[2..]),
        other => {
            eprintln!("Unknown subcommand: {other}");
            std::process::exit(1);
        }
    };

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn cmd_check_tty() {
    use std::io::IsTerminal as _;
    let stdin_tty = if std::io::stdin().is_terminal() { "tty" } else { "not-tty" };
    let stdout_tty = if std::io::stdout().is_terminal() { "tty" } else { "not-tty" };
    let stderr_tty = if std::io::stderr().is_terminal() { "tty" } else { "not-tty" };
    println!("stdin:{stdin_tty}");
    println!("stdout:{stdout_tty}");
    println!("stderr:{stderr_tty}");
}

fn cmd_print(args: &[String]) {
    println!("{}", args.join(" "));
}

fn cmd_print_env(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        return Err("Usage: vtt print-env <VAR_NAME>".into());
    }
    let value = std::env::var(&args[0]).unwrap_or_else(|_| "(undefined)".to_string());
    println!("{value}");
    Ok(())
}

fn cmd_print_file(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for file in args {
        match std::fs::read(file) {
            Ok(content) => out.write_all(&content)?,
            Err(_) => eprintln!("{file}: not found"),
        }
    }
    Ok(())
}

fn cmd_read_stdin() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read as _, Write as _};
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut buf = [0u8; 8192];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => stdout.write_all(&buf[..n])?,
        }
    }
    Ok(())
}

fn cmd_replace_file_content(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 3 {
        return Err("Usage: vtt replace-file-content <filename> <searchValue> <newValue>".into());
    }
    let filename = &args[0];
    let search_value = &args[1];
    let new_value = &args[2];

    let filepath = std::path::Path::new(filename).canonicalize()?;
    let content = std::fs::read_to_string(&filepath)?;
    let new_content = content.replacen(search_value, new_value, 1);
    std::fs::write(&filepath, new_content)?;
    Ok(())
}

fn cmd_touch_file(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        return Err("Usage: vtt touch-file <filename>".into());
    }
    let _file = std::fs::OpenOptions::new().read(true).write(true).open(&args[0])?;
    Ok(())
}

// ── json-edit implementation ──

fn cmd_json_edit(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 2 {
        return Err("Usage: vtt json-edit <filename> <expression>".into());
    }
    let filename = &args[0];
    let expr = &args[1];

    let content = std::fs::read_to_string(filename)?;
    let mut json: serde_json::Value = serde_json::from_str(&content)?;

    let expr = expr.trim();
    if let Some(path_str) = expr.strip_prefix("delete ") {
        let path = parse_path(path_str.trim())?;
        delete_path(&mut json, &path)?;
    } else if let Some((path_str, value_str)) = expr.split_once('=') {
        let path = parse_path(path_str.trim())?;
        let value = parse_value(value_str.trim())?;
        set_path(&mut json, &path, value)?;
    } else {
        return Err(format!("Unsupported expression: {expr}").into());
    }

    let output = serde_json::to_string_pretty(&json)? + "\n";
    std::fs::write(filename, output)?;
    Ok(())
}

fn parse_path(s: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Parse paths like: _.tasks.test.untrackedEnv or _.tasks['empty-inputs'].command
    let s = s.strip_prefix("_.").ok_or("Path must start with '_.' ")?;
    let mut keys = Vec::new();
    let mut chars = s.chars().peekable();
    let mut current = String::new();

    while let Some(&ch) = chars.peek() {
        match ch {
            '.' => {
                if !current.is_empty() {
                    keys.push(std::mem::take(&mut current));
                }
                chars.next();
            }
            '[' => {
                if !current.is_empty() {
                    keys.push(std::mem::take(&mut current));
                }
                chars.next(); // consume '['
                // expect quote
                let quote = chars.next().ok_or("Expected quote after [")?;
                if quote != '\'' && quote != '"' {
                    return Err("Expected quote after [".into());
                }
                let mut key = String::new();
                for ch in chars.by_ref() {
                    if ch == quote {
                        break;
                    }
                    key.push(ch);
                }
                // expect ']'
                let close = chars.next().ok_or("Expected ]")?;
                if close != ']' {
                    return Err("Expected ]".into());
                }
                keys.push(key);
            }
            _ => {
                current.push(ch);
                chars.next();
            }
        }
    }
    if !current.is_empty() {
        keys.push(current);
    }
    Ok(keys)
}

fn parse_value(s: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    // Handle quoted strings
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        let inner = &s[1..s.len() - 1];
        return Ok(serde_json::Value::String(inner.to_string()));
    }
    // Handle arrays like ['foo', 'bar'] or ["foo"]
    if s.starts_with('[') && s.ends_with(']') {
        let inner = s[1..s.len() - 1].trim();
        if inner.is_empty() {
            return Ok(serde_json::Value::Array(vec![]));
        }
        let mut items = Vec::new();
        let mut chars = inner.chars().peekable();
        while chars.peek().is_some() {
            // skip whitespace
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            if chars.peek().is_none() {
                break;
            }
            let quote = chars.next().ok_or("Expected quote")?;
            if quote != '\'' && quote != '"' {
                return Err(format!("Expected quote in array, got '{quote}'").into());
            }
            let mut val = String::new();
            for ch in chars.by_ref() {
                if ch == quote {
                    break;
                }
                val.push(ch);
            }
            items.push(serde_json::Value::String(val));
            // skip whitespace and comma
            while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
                chars.next();
            }
        }
        return Ok(serde_json::Value::Array(items));
    }
    Err(format!("Cannot parse value: {s}").into())
}

fn set_path(
    root: &mut serde_json::Value,
    path: &[String],
    value: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let (last, parents) = path.split_last().ok_or("Empty path")?;
    let mut current = root;
    for key in parents {
        current = current.get_mut(key.as_str()).ok_or_else(|| format!("Key not found: {key}"))?;
    }
    let obj = current.as_object_mut().ok_or("Parent is not an object")?;
    obj.insert(last.clone(), value);
    Ok(())
}

fn delete_path(
    root: &mut serde_json::Value,
    path: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let (last, parents) = path.split_last().ok_or("Empty path")?;
    let mut current = root;
    for key in parents {
        current = current.get_mut(key.as_str()).ok_or_else(|| format!("Key not found: {key}"))?;
    }
    let obj = current.as_object_mut().ok_or("Parent is not an object")?;
    obj.remove(last.as_str());
    Ok(())
}
