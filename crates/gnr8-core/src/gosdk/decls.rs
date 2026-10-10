//! The package-level names one Go file declares.
//!
//! The emitter writes Go as text, so a name it declares is only known once that text exists. This
//! reads it back with a lexer just large enough for that: it skips comments, string and rune
//! literals, tracks bracket depth, and collects the names `func`, `var`, `const` and `type` declare
//! at depth zero. Methods are skipped, because a method name is not a package-level name.

use std::collections::BTreeSet;

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Ident(String),
    Open,
    Close,
    Comma,
    Newline,
    Other,
}

/// The package-level names `source` declares.
pub(crate) fn package_names(source: &str) -> BTreeSet<String> {
    let tokens = tokenize(source);
    let mut names = BTreeSet::new();
    let mut depth = 0usize;
    let mut index = 0;
    while index < tokens.len() {
        match &tokens[index] {
            Token::Open => depth += 1,
            Token::Close => depth = depth.saturating_sub(1),
            Token::Ident(word) if depth == 0 => match word.as_str() {
                "func" => {
                    if let Some(Token::Ident(name)) = tokens.get(index + 1) {
                        names.insert(name.clone());
                    }
                }
                "var" | "const" | "type" => {
                    if tokens.get(index + 1) == Some(&Token::Open) {
                        index = group_names(&tokens, index + 2, &mut names);
                        continue;
                    }
                    spec_names(&tokens, index + 1, &mut names);
                }
                _ => {}
            },
            _ => {}
        }
        index += 1;
    }
    names
}

/// Collect the names of a parenthesized `var`/`const`/`type` group whose body starts at `start`,
/// one spec per line, and return the index after its closing parenthesis.
fn group_names(tokens: &[Token], start: usize, names: &mut BTreeSet<String>) -> usize {
    let mut depth = 1usize;
    let mut spec_start = true;
    let mut index = start;
    while let Some(token) = tokens.get(index) {
        match token {
            Token::Open => depth += 1,
            Token::Close => {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
            Token::Newline if depth == 1 => spec_start = true,
            Token::Ident(_) if depth == 1 && spec_start => {
                index = spec_names(tokens, index, names);
                spec_start = false;
                continue;
            }
            _ => {}
        }
        if !matches!(token, Token::Newline) {
            spec_start = false;
        }
        index += 1;
    }
    index
}

/// Collect the comma-separated names a spec starting at `start` declares, and return the index
/// after the last one.
fn spec_names(tokens: &[Token], start: usize, names: &mut BTreeSet<String>) -> usize {
    let mut index = start;
    while let Some(Token::Ident(name)) = tokens.get(index) {
        names.insert(name.clone());
        index += 1;
        if tokens.get(index) != Some(&Token::Comma) {
            break;
        }
        index += 1;
    }
    index
}

fn tokenize(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while let Some(&ch) = chars.get(index) {
        match ch {
            '/' if chars.get(index + 1) == Some(&'/') => {
                while chars.get(index).is_some_and(|&c| c != '\n') {
                    index += 1;
                }
                continue;
            }
            '/' if chars.get(index + 1) == Some(&'*') => {
                index += 2;
                let mut newline = false;
                while index < chars.len()
                    && !(chars[index] == '*' && chars.get(index + 1) == Some(&'/'))
                {
                    newline |= chars[index] == '\n';
                    index += 1;
                }
                index += 2;
                if newline {
                    tokens.push(Token::Newline);
                }
                continue;
            }
            '"' | '\'' => {
                index += 1;
                while let Some(&c) = chars.get(index) {
                    index += 1;
                    if c == '\\' {
                        index += 1;
                    } else if c == ch {
                        break;
                    }
                }
                tokens.push(Token::Other);
                continue;
            }
            '`' => {
                index += 1;
                while chars.get(index).is_some_and(|&c| c != '`') {
                    index += 1;
                }
                index += 1;
                tokens.push(Token::Other);
                continue;
            }
            '{' | '(' | '[' => tokens.push(Token::Open),
            '}' | ')' | ']' => tokens.push(Token::Close),
            ',' => tokens.push(Token::Comma),
            '\n' => tokens.push(Token::Newline),
            c if c == '_' || c.is_ascii_alphanumeric() => {
                let start = index;
                while chars
                    .get(index)
                    .is_some_and(|&c| c == '_' || c.is_ascii_alphanumeric())
                {
                    index += 1;
                }
                if c.is_ascii_digit() {
                    tokens.push(Token::Other);
                } else {
                    tokens.push(Token::Ident(chars[start..index].iter().collect()));
                }
                continue;
            }
            c if c.is_whitespace() => {}
            _ => tokens.push(Token::Other),
        }
        index += 1;
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::package_names;

    fn names(source: &str) -> Vec<String> {
        package_names(source).into_iter().collect()
    }

    #[test]
    fn collects_functions_vars_consts_and_types() {
        let source = r#"package cli

import (
	"fmt"
)

var active Options

var cliGroups = []cliGroup{
	{name: "books", commands: []cliCommand{{name: "list"}}},
}

const (
	program = "bookstore"
	first, second = 1, 2
)

type cliCommand struct {
	name string
}

func (c cliCommand) String() string { return c.name }

func complete(args []string) int {
	var local int
	switch v := any(local).(type) {
	default:
		_ = v
	}
	return 0
}

var handler = func() {}
"#;
        assert_eq!(
            names(source),
            [
                "active",
                "cliCommand",
                "cliGroups",
                "complete",
                "first",
                "handler",
                "program",
                "second"
            ]
        );
    }

    #[test]
    fn ignores_names_inside_literals_and_comments() {
        let source = "package cli\n\
            var text = `\nfunc countBooks() {\n`\n\
            var hint = \"Did you mean `%s`? func notThis(\"\n\
            var tick = '`'\n\
            // func commented() {}\n\
            /* var hidden int\n */\n\
            func real() {}\n";
        assert_eq!(names(source), ["hint", "real", "text", "tick"]);
    }
}
