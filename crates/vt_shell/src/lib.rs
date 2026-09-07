use std::{collections::BTreeMap, fmt::Display, ops::Range};

use brush_parser::{
    Parser, ParserImpl, ParserOptions,
    ast::{
        AndOr, Assignment, AssignmentName, AssignmentValue, Command, CommandPrefix,
        CommandPrefixOrSuffixItem, CommandSuffix, CompoundListItem, Pipeline, Program,
        SeparatorOperator, SimpleCommand, SourceLocation, Word,
    },
    word::{WordPiece, WordPieceWithSource},
};
use diff::Diff;
use serde::{Deserialize, Serialize};
use vt_str::Str;
use wincode::{SchemaRead, SchemaWrite};

/// "FOO=BAR program arg1 arg2"
#[derive(SchemaWrite, SchemaRead, Serialize, Deserialize, Debug, PartialEq, Eq, Diff, Clone)]
#[diff(attr(#[derive(Debug)]))]
pub struct TaskParsedCommand {
    pub envs: BTreeMap<Str, Str>,
    pub program: Str,
    pub args: Vec<Str>,
}

impl Display for TaskParsedCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // BTreeMap ensures stable iteration order
        for (name, value) in &self.envs {
            Display::fmt(
                &format_args!("{}={} ", name, shell_escape::escape(value.as_str().into())),
                f,
            )?;
        }
        Display::fmt(&shell_escape::escape(self.program.as_str().into()), f)?;
        for arg in &self.args {
            Display::fmt(" ", f)?;
            Display::fmt(&shell_escape::escape(arg.as_str().into()), f)?;
        }

        Ok(())
    }
}

/// Parser options matching those used in [`try_parse_as_and_list`].
const PARSER_OPTIONS: ParserOptions = ParserOptions {
    enable_extended_globbing: false,
    posix_mode: true,
    sh_mode: true,
    tilde_expansion_at_word_start: false,
    tilde_expansion_after_colon: false,
    parser_impl: ParserImpl::Peg,
};

/// Remove shell quoting from a word value, respecting quoting context.
///
/// Uses `brush_parser::word::parse` to properly handle nested quoting
/// (e.g. single quotes inside double quotes are preserved as literal characters).
/// Returns `None` if the word contains expansions that cannot be statically resolved
/// (pathname expansion when enabled, parameter expansion, command substitution, arithmetic).
fn unquote(word: &Word, pathname_expansion_enabled: bool) -> Option<Str> {
    let Word { value, loc: _ } = word;
    let pieces = brush_parser::word::parse(value.as_str(), &PARSER_OPTIONS).ok()?;
    if pathname_expansion_enabled && contains_pathname_expansion(&pieces) {
        return None;
    }
    let mut result = Str::with_capacity(value.len());
    flatten_pieces(&pieces, &mut result)?;
    Some(result)
}

#[derive(Default)]
struct PathnameExpansionDetector {
    bracket_expression: Option<BracketExpression>,
}

#[derive(Default)]
struct BracketExpression {
    has_member: bool,
    can_negate: bool,
}

impl PathnameExpansionDetector {
    fn push(&mut self, value: &str, pattern_syntax_enabled: bool) -> bool {
        for char in value.chars() {
            if char == '/' {
                // A bracket expression cannot cross a pathname component boundary.
                self.bracket_expression = None;
                continue;
            }

            let Some(bracket_expression) = &mut self.bracket_expression else {
                if pattern_syntax_enabled {
                    if matches!(char, '*' | '?') {
                        return true;
                    }
                    if char == '[' {
                        self.bracket_expression =
                            Some(BracketExpression { has_member: false, can_negate: true });
                    }
                }
                continue;
            };

            if pattern_syntax_enabled && char == ']' {
                if bracket_expression.has_member {
                    return true;
                }
                // `]` is a literal member when it is the first character after `[` or
                // an optional negation character. A later unquoted `]` must still close it.
                bracket_expression.has_member = true;
            } else if pattern_syntax_enabled
                && bracket_expression.can_negate
                && matches!(char, '!' | '^')
            {
            } else {
                bracket_expression.has_member = true;
            }
            bracket_expression.can_negate = false;
        }
        false
    }
}

fn contains_pathname_expansion(pieces: &[WordPieceWithSource]) -> bool {
    fn visit(
        pieces: &[WordPieceWithSource],
        detector: &mut PathnameExpansionDetector,
        pattern_syntax_enabled: bool,
    ) -> bool {
        for piece in pieces {
            let found = match &piece.piece {
                WordPiece::Text(s) => detector.push(s, pattern_syntax_enabled),
                WordPiece::SingleQuotedText(s) | WordPiece::AnsiCQuotedText(s) => {
                    detector.push(s, false)
                }
                WordPiece::EscapeSequence(s) => {
                    detector.push(s.strip_prefix('\\').unwrap_or(s), false)
                }
                WordPiece::DoubleQuotedSequence(inner)
                | WordPiece::GettextDoubleQuotedSequence(inner) => visit(inner, detector, false),
                _ => false,
            };
            if found {
                return true;
            }
        }
        false
    }

    visit(pieces, &mut PathnameExpansionDetector::default(), true)
}

/// Recursively extract literal text from parsed word pieces.
///
/// Returns `None` if any piece requires runtime expansion.
fn flatten_pieces(pieces: &[WordPieceWithSource], result: &mut Str) -> Option<()> {
    for piece in pieces {
        match &piece.piece {
            WordPiece::Text(s) | WordPiece::SingleQuotedText(s) | WordPiece::AnsiCQuotedText(s) => {
                result.push_str(s);
            }
            // EscapeSequence contains the raw sequence (e.g. `\"` as two chars);
            // the escaped character is everything after the leading backslash.
            WordPiece::EscapeSequence(s) => {
                result.push_str(s.strip_prefix('\\').unwrap_or(s));
            }
            WordPiece::DoubleQuotedSequence(inner)
            | WordPiece::GettextDoubleQuotedSequence(inner) => {
                flatten_pieces(inner, result)?;
            }
            // Tilde prefix, parameter expansion, command substitution, arithmetic
            // cannot be statically resolved — bail out.
            _ => return None,
        }
    }
    Some(())
}

fn pipeline_to_command(
    pipeline: &Pipeline,
    pathname_expansion_enabled: bool,
) -> Option<(TaskParsedCommand, Range<usize>)> {
    let location = pipeline.location()?;
    let range = location.start.index..location.end.index;

    let Pipeline { timed: None, bang: false, seq } = pipeline else {
        return None;
    };
    let [Command::Simple(simple_command)] = seq.as_slice() else {
        return None;
    };
    let SimpleCommand { prefix, word_or_name: Some(program), suffix } = simple_command else {
        return None;
    };
    let mut envs = BTreeMap::<Str, Str>::new();
    if let Some(prefix) = prefix {
        let CommandPrefix(items) = prefix;
        for item in items {
            let CommandPrefixOrSuffixItem::AssignmentWord(
                Assignment { name, value, append: false, loc: _ },
                _,
            ) = item
            else {
                return None;
            };
            let AssignmentName::VariableName(name) = name else {
                return None;
            };
            let AssignmentValue::Scalar(value) = value else {
                return None;
            };
            // Assignment values are not subject to pathname expansion.
            envs.insert(name.as_str().into(), unquote(value, false)?);
        }
    }
    let mut args = Vec::<Str>::new();
    if let Some(CommandSuffix(suffix_items)) = suffix {
        for suffix_item in suffix_items {
            let CommandPrefixOrSuffixItem::Word(word) = suffix_item else {
                return None;
            };
            args.push(unquote(word, pathname_expansion_enabled)?);
        }
    }
    Some((
        TaskParsedCommand { envs, program: unquote(program, pathname_expansion_enabled)?, args },
        range,
    ))
}

/// Parses commands that can be executed without a shell.
///
/// Set `pathname_expansion_enabled` when the target shell treats unquoted patterns as pathname
/// expansions. Such patterns make the command ineligible for static execution.
#[must_use]
pub fn try_parse_as_and_list(
    cmd: &str,
    pathname_expansion_enabled: bool,
) -> Option<Vec<(TaskParsedCommand, Range<usize>)>> {
    let mut parser = Parser::new(cmd.as_bytes(), &PARSER_OPTIONS);
    let Program { complete_commands } = parser.parse_program().ok()?;
    let [compound_list] = complete_commands.as_slice() else {
        return None;
    };
    let [CompoundListItem(and_or_list, SeparatorOperator::Sequence)] = compound_list.0.as_slice()
    else {
        return None;
    };

    let mut commands = Vec::<(TaskParsedCommand, Range<usize>)>::new();
    commands.push(pipeline_to_command(&and_or_list.first, pathname_expansion_enabled)?);
    for and_or in &and_or_list.additional {
        let AndOr::And(pipeline) = and_or else {
            return None;
        };
        commands.push(pipeline_to_command(pipeline, pathname_expansion_enabled)?);
    }
    Some(commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(cmd: &str) -> Option<Vec<(TaskParsedCommand, Range<usize>)>> {
        try_parse_as_and_list(cmd, true)
    }

    #[test]
    fn test_parse_single_command() {
        let source = r"A=B hello world";
        let list = parse(source).unwrap();
        assert_eq!(list.len(), 1);
        let (cmd, range) = &list[0];
        assert_eq!(&source[range.clone()], source);
        assert_eq!(
            cmd,
            &TaskParsedCommand {
                envs: [("A".into(), "B".into())].into(),
                program: "hello".into(),
                args: vec!["world".into()],
            }
        );
    }

    #[test]
    fn test_parse_command() {
        let source = r#"A=B hello world && FOO="BE\"R" program "arg1" "arg\"2" && zzz"#;
        let list = parse(source).unwrap();

        let commands = list.iter().map(|(cmd, _)| cmd).collect::<Vec<_>>();
        assert_eq!(
            commands,
            vec![
                &TaskParsedCommand {
                    envs: [("A".into(), "B".into())].into(),
                    program: "hello".into(),
                    args: vec!["world".into()],
                },
                &TaskParsedCommand {
                    envs: [("FOO".into(), "BE\"R".into())].into(),
                    program: "program".into(),
                    args: vec!["arg1".into(), "arg\"2".into()],
                },
                &TaskParsedCommand { envs: [].into(), program: "zzz".into(), args: vec![] }
            ]
        );

        let substrs = list.iter().map(|(_, range)| &source[range.clone()]).collect::<Vec<_>>();

        assert_eq!(
            substrs,
            vec!["A=B hello world", r#"FOO="BE\"R" program "arg1" "arg\"2""#, "zzz"]
        );
    }

    #[test]
    fn test_task_parsed_command_stable_env_ordering() {
        // Test that environment variables maintain stable ordering
        let cmd = TaskParsedCommand {
            envs: [
                ("ZEBRA".into(), "last".into()),
                ("ALPHA".into(), "first".into()),
                ("MIDDLE".into(), "middle".into()),
            ]
            .into(),
            program: "test".into(),
            args: vec![],
        };

        // Convert to string multiple times and verify it's always the same
        let str1 = cmd.to_string();
        let str2 = cmd.to_string();
        let str3 = cmd.to_string();

        assert_eq!(str1, str2);
        assert_eq!(str2, str3);

        // Verify the order is alphabetical (BTreeMap sorts by key)
        assert!(str1.starts_with("ALPHA=first MIDDLE=middle ZEBRA=last"));
    }

    #[test]
    fn test_unquote_preserves_nested_quotes() {
        // Single quotes inside double quotes are preserved
        let cmd = r#"echo "hello 'world'""#;
        let list = parse(cmd).unwrap();
        assert_eq!(list[0].0.args[0].as_str(), "hello 'world'");

        // Double quotes inside single quotes are preserved
        let cmd = r#"echo 'hello "world"'"#;
        let list = parse(cmd).unwrap();
        assert_eq!(list[0].0.args[0].as_str(), "hello \"world\"");

        // Backslash escaping in double quotes
        let cmd = r#"echo "hello\"world""#;
        let list = parse(cmd).unwrap();
        assert_eq!(list[0].0.args[0].as_str(), "hello\"world");

        // Backslash escaping outside quotes
        let cmd = r"echo hello\ world";
        let list = parse(cmd).unwrap();
        assert_eq!(list[0].0.args[0].as_str(), "hello world");
    }

    #[test]
    fn test_flatten_pieces_recursion() {
        fn parse_and_flatten(input: &str) -> Option<Str> {
            let pieces = brush_parser::word::parse(input, &PARSER_OPTIONS).ok()?;
            let mut result = Str::default();
            flatten_pieces(&pieces, &mut result)?;
            Some(result)
        }

        // DoubleQuotedSequence containing Text + EscapeSequence + Text
        assert_eq!(parse_and_flatten(r#""hello\"world""#).unwrap(), "hello\"world");

        // DoubleQuotedSequence with single quotes preserved as literal text
        assert_eq!(parse_and_flatten(r#""it's a 'test'""#).unwrap(), "it's a 'test'");

        // Nested escape sequences inside double quotes
        assert_eq!(parse_and_flatten(r#""a\\b""#).unwrap(), "a\\b");

        // DoubleQuotedSequence bails on parameter expansion inside
        assert!(parse_and_flatten(r#""hello $VAR""#).is_none());

        // DoubleQuotedSequence bails on command substitution inside
        assert!(parse_and_flatten(r#""hello $(cmd)""#).is_none());
    }

    #[test]
    fn test_unquoted_pathname_expansion_uses_shell_fallback() {
        for cmd in [
            "tool packages/*/src",
            "tool packages/?/src",
            "tool packages/[ab]/src",
            "tool packages/[!ab]/src",
            "tool packages/[]a]/src",
            "tool packages/[[:alpha:]]/src",
            "packages/*/bin --help",
            "tool --pattern=*",
            "tool https://example.test/items?limit=1",
            "tool expression[ab]",
        ] {
            assert!(parse(cmd).is_none(), "{cmd}");
        }
    }

    #[test]
    fn test_pathname_expansion_fallback_is_not_path_heuristic() {
        // Shell expansion is determined by unquoted word syntax, not by whether a word looks
        // like a filesystem path. These non-path words must still use the shell so their
        // behavior matches package-manager scripts.
        for cmd in ["tool --include=*", "tool key?value", "tool selector[ab]"] {
            assert!(parse(cmd).is_none(), "{cmd}");
        }
    }

    #[test]
    fn test_shell_without_pathname_expansion_keeps_patterns_on_static_path() {
        let parsed =
            try_parse_as_and_list("tool packages/*/src && tool packages/[ab]/src", false).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0.args[0], "packages/*/src");
        assert_eq!(parsed[1].0.args[0], "packages/[ab]/src");
    }

    #[test]
    fn test_unmatched_bracket_stays_on_static_path() {
        // An unmatched `[` is an ordinary character rather than a bracket expression. A slash
        // also terminates the pathname component before a later `]` can close the expression.
        for (cmd, expected) in [
            ("tool selector[abc", "selector[abc"),
            ("tool packages/[abc/src]", "packages/[abc/src]"),
            ("tool selector[]", "selector[]"),
            ("tool selector[!]", "selector[!]"),
        ] {
            let parsed = parse(cmd).unwrap();
            assert_eq!(parsed[0].0.args[0], expected);
        }
    }

    #[test]
    fn test_glob_in_and_list_falls_back_as_one_shell_script() {
        assert!(parse("tool before && tool packages/*/src").is_none());
        assert!(parse("tool packages/*/src && tool after").is_none());

        let parsed = parse("tool before && tool after").unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn test_invalid_shell_syntax_uses_shell_fallback() {
        assert!(parse("tool 'unterminated").is_none());
    }

    #[test]
    fn test_quoted_or_escaped_pathname_patterns_stay_literal() {
        for (cmd, expected) in [
            (r#"tool "packages/*/src""#, "packages/*/src"),
            ("tool 'packages/?/src'", "packages/?/src"),
            (r"tool packages/\[ab\]/src", "packages/[ab]/src"),
            (r#"tool "https://example.test/items?limit=1""#, "https://example.test/items?limit=1"),
            (r"tool --pattern=\*", "--pattern=*"),
        ] {
            let parsed = parse(cmd).unwrap();
            assert_eq!(parsed[0].0.args[0], expected);
        }
    }

    #[test]
    fn test_assignment_value_pathname_patterns_stay_literal() {
        let parsed = parse("PATTERN=* tool").unwrap();
        assert_eq!(parsed[0].0.envs["PATTERN"], "*");
    }

    #[test]
    fn test_parse_urllib_prepare() {
        let cmd = r#"node -e "const v = parseInt(process.versions.node, 10); if (v >= 20) require('child_process').execSync('vp config', {stdio: 'inherit'});""#;
        let result = parse(cmd);
        let (parsed, _) = &result.as_ref().unwrap()[0];
        // Single quotes inside double quotes must be preserved as literal characters
        assert_eq!(
            parsed.args[1].as_str(),
            "const v = parseInt(process.versions.node, 10); if (v >= 20) require('child_process').execSync('vp config', {stdio: 'inherit'});"
        );
    }

    #[test]
    fn test_task_parsed_command_serialization_stability() {
        // Create a command with multiple environment variables
        let cmd = TaskParsedCommand {
            envs: [
                ("VAR_C".into(), "value_c".into()),
                ("VAR_A".into(), "value_a".into()),
                ("VAR_B".into(), "value_b".into()),
            ]
            .into(),
            program: "program".into(),
            args: vec!["arg1".into(), "arg2".into()],
        };

        // Serialize multiple times
        let bytes1 = wincode::serialize(&cmd).unwrap();
        let bytes2 = wincode::serialize(&cmd).unwrap();

        // Verify serialization is stable
        assert_eq!(bytes1, bytes2);

        // Verify deserialization works and maintains order
        let decoded: TaskParsedCommand = wincode::deserialize(&bytes1).unwrap();
        assert_eq!(decoded, cmd);

        // Verify the decoded command still has stable string representation
        assert_eq!(decoded.to_string(), cmd.to_string());
    }
}
