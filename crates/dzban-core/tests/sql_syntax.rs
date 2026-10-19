use dzban_core::sql::{SqlLexer, SqlTokenKind};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    schema: u32,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    source: String,
    tokens: Vec<Token>,
}
#[derive(Debug, PartialEq, Deserialize)]
struct Token {
    start: usize,
    length: usize,
    kind: SqlTokenKind,
}

#[test]
fn shared_tokens_match_the_frozen_native_lexer() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("../../../tests/fixtures/sql-syntax-v1.json")).unwrap();
    assert_eq!(fixture.schema, 1);
    for case in fixture.cases {
        let tokens: Vec<_> = SqlLexer::new(&case.source, 1)
            .map(|token| Token {
                start: token.start.utf16,
                length: token.end.utf16 - token.start.utf16,
                kind: token.kind,
            })
            .collect();
        assert_eq!(tokens, case.tokens, "{}", case.name);
    }
}

#[test]
fn every_offset_unit_addresses_the_same_unicode_source() {
    let source = "😀(é), SELECT 'žluť\r\n😀', 1 -- پایان";
    let scalars: Vec<_> = source.chars().collect();
    let utf16: Vec<_> = source.encode_utf16().collect();
    for token in SqlLexer::new(source, 1) {
        let bytes = &source[token.start.bytes..token.end.bytes];
        assert_eq!(
            bytes,
            scalars[token.start.scalars..token.end.scalars]
                .iter()
                .collect::<String>()
        );
        assert_eq!(
            bytes,
            String::from_utf16(&utf16[token.start.utf16..token.end.utf16]).unwrap()
        );
    }
}

#[test]
fn batches_are_lossless_bounded_and_keep_the_exact_revision_and_length() {
    let source = "SELECT '😀', count(x), 1; -- text\r\n".repeat(1000);
    let expected: Vec<_> = SqlLexer::new(&source, 1).collect();
    let length = dzban_core::sql::TextOffset {
        bytes: source.len(),
        scalars: source.chars().count(),
        utf16: source.encode_utf16().count(),
    };
    for size in [1, 17, 512, 4096] {
        let mut lexer = SqlLexer::new(&source, u64::MAX);
        let mut tokens = Vec::new();
        loop {
            let batch = lexer.read(size);
            assert_eq!(batch.revision, u64::MAX);
            assert_eq!(batch.length, length);
            assert!(batch.tokens.len() <= size);
            tokens.extend(batch.tokens);
            if batch.done {
                break;
            }
        }
        assert_eq!(tokens, expected);
        assert!(lexer.read(size).tokens.is_empty());
        assert!(lexer.read(size).done);
    }
}
