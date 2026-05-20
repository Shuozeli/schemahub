use schemahub_types::errors::ParseError;

use crate::ast::{EnumBlob, EnumValueDef, FieldDef, FileMetadataBlob, StructBlob, StructFieldDef, TableBlob, UnionBlob};

// ── Public output ─────────────────────────────────────────────────────────────

pub struct ParsedFile {
    pub metadata: FileMetadataBlob,
    pub tables: Vec<TableBlob>,
    pub structs: Vec<StructBlob>,
    pub enums: Vec<EnumBlob>,
    pub unions: Vec<UnionBlob>,
}

// ── Tokeniser ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    StringLit(String),
    Colon,
    Semicolon,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Eq,
    Comma,
    Dot,
    Dash,
    LineComment(String),
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self { src: src.as_bytes(), pos: 0, line: 1 }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let ch = self.src.get(self.pos).copied()?;
        self.pos += 1;
        if ch == b'\n' { self.line += 1; }
        Some(ch)
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_ascii_whitespace() { self.advance(); } else { break; }
        }
    }

    fn read_word(&mut self, first: u8) -> String {
        let mut word = vec![first];
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                word.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&word).into_owned()
    }

    fn read_string_lit(&mut self) -> Result<String, ParseError> {
        let mut s = Vec::new();
        loop {
            match self.advance() {
                None => return Err(ParseError::SyntaxError {
                    line: self.line,
                    message: "unterminated string literal".into(),
                }),
                Some(b'"') => break,
                Some(b'\\') => {
                    if let Some(c) = self.advance() {
                        s.push(b'\\');
                        s.push(c);
                    }
                }
                Some(c) => s.push(c),
            }
        }
        Ok(String::from_utf8_lossy(&s).into_owned())
    }

    fn read_line_comment(&mut self) -> String {
        let mut s = Vec::new();
        while let Some(ch) = self.peek() {
            if ch == b'\n' { break; }
            s.push(ch);
            self.advance();
        }
        String::from_utf8_lossy(&s).trim().to_owned()
    }

    fn skip_block_comment(&mut self) {
        loop {
            match self.advance() {
                None => break,
                Some(b'*') => {
                    if self.peek() == Some(b'/') {
                        self.advance();
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    fn tokenise(&mut self) -> Result<Vec<(Token, usize)>, ParseError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace();
            let line = self.line;
            match self.peek() {
                None => break,
                Some(b':') => { self.advance(); tokens.push((Token::Colon, line)); }
                Some(b';') => { self.advance(); tokens.push((Token::Semicolon, line)); }
                Some(b'{') => { self.advance(); tokens.push((Token::LBrace, line)); }
                Some(b'}') => { self.advance(); tokens.push((Token::RBrace, line)); }
                Some(b'[') => { self.advance(); tokens.push((Token::LBracket, line)); }
                Some(b']') => { self.advance(); tokens.push((Token::RBracket, line)); }
                Some(b'(') => { self.advance(); tokens.push((Token::LParen, line)); }
                Some(b')') => { self.advance(); tokens.push((Token::RParen, line)); }
                Some(b'=') => { self.advance(); tokens.push((Token::Eq, line)); }
                Some(b',') => { self.advance(); tokens.push((Token::Comma, line)); }
                Some(b'.') => { self.advance(); tokens.push((Token::Dot, line)); }
                Some(b'-') => { self.advance(); tokens.push((Token::Dash, line)); }
                Some(b'"') => {
                    self.advance();
                    let s = self.read_string_lit()?;
                    tokens.push((Token::StringLit(s), line));
                }
                Some(b'/') => {
                    self.advance();
                    match self.peek() {
                        Some(b'/') => {
                            self.advance();
                            let cmt = self.read_line_comment();
                            tokens.push((Token::LineComment(cmt), line));
                        }
                        Some(b'*') => {
                            self.advance();
                            self.skip_block_comment();
                        }
                        _ => {}
                    }
                }
                Some(ch) if ch.is_ascii_alphanumeric() || ch == b'_' => {
                    let c = self.advance().unwrap();
                    let w = self.read_word(c);
                    tokens.push((Token::Word(w), line));
                }
                Some(_) => { self.advance(); }
            }
        }
        Ok(tokens)
    }
}

// ── Parser ────────────────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<(Token, usize)>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<(Token, usize)>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    fn peek_line(&self) -> usize {
        self.tokens.get(self.pos).map(|(_, l)| *l).unwrap_or(0)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos).map(|(t, _)| t);
        self.pos += 1;
        tok
    }

    fn expect_word(&mut self) -> Result<String, ParseError> {
        let line = self.peek_line();
        match self.advance() {
            Some(Token::Word(w)) => Ok(w.clone()),
            other => Err(ParseError::SyntaxError {
                line,
                message: format!("expected identifier, got {:?}", other),
            }),
        }
    }

    fn expect_semicolon(&mut self) -> Result<(), ParseError> {
        match self.advance() {
            Some(Token::Semicolon) => Ok(()),
            _ => Ok(()), // lenient
        }
    }

    /// Collect consecutive line comments as doc string.
    fn collect_pending_comments(&mut self) -> String {
        let mut parts = Vec::new();
        loop {
            match self.peek() {
                Some(Token::LineComment(_)) => {
                    if let Some(Token::LineComment(c)) = self.advance() {
                        let trimmed = c.trim_start_matches('/').trim().to_owned();
                        if !trimmed.is_empty() {
                            parts.push(trimmed.to_owned());
                        }
                    }
                }
                _ => break,
            }
        }
        parts.join("\n")
    }

    fn skip_block(&mut self) {
        let mut depth = 1usize;
        while let Some(tok) = self.advance() {
            match tok {
                Token::LBrace => depth += 1,
                Token::RBrace => {
                    depth -= 1;
                    if depth == 0 { break; }
                }
                _ => {}
            }
        }
    }

    fn skip_to_semicolon(&mut self) {
        while let Some(tok) = self.advance() {
            match tok {
                Token::Semicolon => break,
                Token::LBrace => { self.skip_block(); }
                _ => {}
            }
        }
    }

    /// Parse a dotted namespace name like "Foo.Bar.Baz"
    fn parse_dotted_name(&mut self) -> Result<String, ParseError> {
        let mut name = self.expect_word()?;
        loop {
            if matches!(self.peek(), Some(Token::Dot)) {
                self.advance();
                let part = self.expect_word()?;
                name.push('.');
                name.push_str(&part);
            } else {
                break;
            }
        }
        Ok(name)
    }

    /// Parse a FlatBuffers type, which may be a vector like `[TypeName]`
    fn parse_type(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some(Token::LBracket) => {
                self.advance();
                let inner = self.expect_word()?;
                // consume ]
                if let Some(Token::RBracket) = self.peek() {
                    self.advance();
                }
                Ok(format!("[{inner}]"))
            }
            _ => self.expect_word(),
        }
    }

    fn parse_number(&mut self) -> Result<i64, ParseError> {
        let negative = matches!(self.peek(), Some(Token::Dash));
        if negative { self.advance(); }
        let line = self.peek_line();
        match self.advance() {
            Some(Token::Word(w)) => {
                let w = w.clone();
                let n: i64 = w.parse().map_err(|_| ParseError::SyntaxError {
                    line,
                    message: format!("expected number, got '{w}'"),
                })?;
                Ok(if negative { -n } else { n })
            }
            other => Err(ParseError::SyntaxError {
                line,
                message: format!("expected number, got {:?}", other),
            }),
        }
    }

    // ── Top-level ─────────────────────────────────────────────────────────────

    fn parse_file(&mut self) -> Result<ParsedFile, ParseError> {
        let mut metadata = FileMetadataBlob::default();
        let mut tables = Vec::new();
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut unions = Vec::new();

        while self.peek().is_some() {
            let doc = self.collect_pending_comments();
            match self.peek() {
                None => break,
                Some(Token::Word(w)) => match w.as_str() {
                    "namespace" => {
                        self.advance();
                        metadata.namespace = self.parse_dotted_name()?;
                        self.expect_semicolon()?;
                    }
                    "include" => {
                        self.advance();
                        let line = self.peek_line();
                        match self.advance() {
                            Some(Token::StringLit(s)) => metadata.imports.push(s.clone()),
                            _ => return Err(ParseError::SyntaxError {
                                line,
                                message: "expected string after 'include'".into(),
                            }),
                        }
                        self.expect_semicolon()?;
                    }
                    "table" => {
                        self.advance();
                        let t = self.parse_table(doc)?;
                        tables.push(t);
                    }
                    "struct" => {
                        self.advance();
                        let s = self.parse_struct(doc)?;
                        structs.push(s);
                    }
                    "enum" => {
                        self.advance();
                        let e = self.parse_enum(doc)?;
                        enums.push(e);
                    }
                    "union" => {
                        self.advance();
                        let u = self.parse_union(doc)?;
                        unions.push(u);
                    }
                    "root_type" => {
                        self.advance();
                        metadata.root_type = self.expect_word()?;
                        self.expect_semicolon()?;
                    }
                    "file_identifier" => {
                        self.advance();
                        let line = self.peek_line();
                        match self.advance() {
                            Some(Token::StringLit(s)) => metadata.file_identifier.push(s.clone()),
                            _ => return Err(ParseError::SyntaxError {
                                line,
                                message: "expected string after 'file_identifier'".into(),
                            }),
                        }
                        self.expect_semicolon()?;
                    }
                    "attribute" => {
                        self.skip_to_semicolon();
                    }
                    _ => {
                        self.skip_to_semicolon();
                    }
                },
                Some(Token::Semicolon) => { self.advance(); }
                _ => { self.advance(); }
            }
        }

        Ok(ParsedFile { metadata, tables, structs, enums, unions })
    }

    // ── Table ─────────────────────────────────────────────────────────────────

    fn parse_table(&mut self, doc_comment: String) -> Result<TableBlob, ParseError> {
        let name = self.expect_word()?;

        match self.advance() {
            Some(Token::LBrace) => {}
            _ => return Err(ParseError::SyntaxError {
                line: self.peek_line(),
                message: format!("expected '{{' after table name '{name}'"),
            }),
        }

        let mut fields = Vec::new();
        let mut slot_counter: u32 = 0;

        loop {
            let field_doc = self.collect_pending_comments();
            match self.peek() {
                None => break,
                Some(Token::RBrace) => { self.advance(); break; }
                Some(Token::Semicolon) => { self.advance(); continue; }
                Some(Token::Word(_)) => {
                    if let Some(f) = self.parse_table_field(field_doc, slot_counter)? {
                        slot_counter += 1;
                        fields.push(f);
                    }
                }
                _ => { self.advance(); }
            }
        }

        Ok(TableBlob { name, fields, doc_comment })
    }

    fn parse_table_field(
        &mut self,
        doc_comment: String,
        slot_index: u32,
    ) -> Result<Option<FieldDef>, ParseError> {
        let name = match self.advance() {
            Some(Token::Word(w)) => w.clone(),
            _ => return Ok(None),
        };

        // expect ':'
        match self.advance() {
            Some(Token::Colon) => {}
            _ => { self.skip_to_semicolon(); return Ok(None); }
        }

        let field_type = self.parse_type()?;

        // optional default: = value
        let mut default_value = String::new();
        if matches!(self.peek(), Some(Token::Eq)) {
            self.advance();
            // could be a word (identifier / number / negative number)
            let negative = matches!(self.peek(), Some(Token::Dash));
            if negative { self.advance(); }
            match self.advance() {
                Some(Token::Word(w)) => {
                    default_value = if negative { format!("-{w}") } else { w.clone() };
                }
                _ => {}
            }
        }

        // optional attributes (deprecated)
        let mut deprecated = false;
        if matches!(self.peek(), Some(Token::LParen)) {
            self.advance();
            // parse comma-separated attributes
            loop {
                match self.peek() {
                    Some(Token::RParen) => { self.advance(); break; }
                    Some(Token::Comma) => { self.advance(); }
                    Some(Token::Word(w)) if w == "deprecated" => {
                        deprecated = true;
                        self.advance();
                    }
                    Some(Token::Word(_)) => { self.advance(); }
                    _ => { self.advance(); break; }
                }
            }
        }

        // expect semicolon
        match self.peek() {
            Some(Token::Semicolon) => { self.advance(); }
            _ => {}
        }

        Ok(Some(FieldDef {
            name,
            field_type,
            default_value,
            deprecated,
            slot_index,
            doc_comment,
        }))
    }

    // ── Struct ────────────────────────────────────────────────────────────────

    fn parse_struct(&mut self, doc_comment: String) -> Result<StructBlob, ParseError> {
        let name = self.expect_word()?;

        match self.advance() {
            Some(Token::LBrace) => {}
            _ => return Err(ParseError::SyntaxError {
                line: self.peek_line(),
                message: format!("expected '{{' after struct name '{name}'"),
            }),
        }

        let mut fields = Vec::new();

        loop {
            let field_doc = self.collect_pending_comments();
            match self.peek() {
                None => break,
                Some(Token::RBrace) => { self.advance(); break; }
                Some(Token::Semicolon) => { self.advance(); continue; }
                Some(Token::Word(_)) => {
                    let fname = self.expect_word()?;
                    match self.advance() {
                        Some(Token::Colon) => {}
                        _ => { self.skip_to_semicolon(); continue; }
                    }
                    let ftype = self.expect_word()?;
                    self.skip_to_semicolon();
                    fields.push(StructFieldDef {
                        name: fname,
                        field_type: ftype,
                        doc_comment: field_doc,
                    });
                }
                _ => { self.advance(); }
            }
        }

        Ok(StructBlob { name, fields, doc_comment })
    }

    // ── Enum ──────────────────────────────────────────────────────────────────

    fn parse_enum(&mut self, doc_comment: String) -> Result<EnumBlob, ParseError> {
        let name = self.expect_word()?;

        // expect : base_type
        let base_type = match self.advance() {
            Some(Token::Colon) => self.expect_word()?,
            _ => "int32".to_owned(), // default
        };

        match self.advance() {
            Some(Token::LBrace) => {}
            _ => return Err(ParseError::SyntaxError {
                line: self.peek_line(),
                message: format!("expected '{{' after enum base type in '{name}'"),
            }),
        }

        let mut values = Vec::new();
        let mut next_value: i64 = 0;

        loop {
            let vdoc = self.collect_pending_comments();
            match self.peek() {
                None => break,
                Some(Token::RBrace) => { self.advance(); break; }
                Some(Token::Semicolon) => { self.advance(); continue; }
                Some(Token::Comma) => { self.advance(); continue; }
                Some(Token::Word(_)) => {
                    let value_name = self.expect_word()?;
                    let value = if matches!(self.peek(), Some(Token::Eq)) {
                        self.advance();
                        let v = self.parse_number()?;
                        next_value = v + 1;
                        v
                    } else {
                        let v = next_value;
                        next_value += 1;
                        v
                    };
                    // skip comma or semicolon
                    if matches!(self.peek(), Some(Token::Comma)) {
                        self.advance();
                    }
                    values.push(EnumValueDef { name: value_name, value, doc_comment: vdoc });
                }
                _ => { self.advance(); }
            }
        }

        Ok(EnumBlob { name, base_type, values, doc_comment })
    }

    // ── Union ─────────────────────────────────────────────────────────────────

    fn parse_union(&mut self, doc_comment: String) -> Result<UnionBlob, ParseError> {
        let name = self.expect_word()?;

        match self.advance() {
            Some(Token::LBrace) => {}
            _ => return Err(ParseError::SyntaxError {
                line: self.peek_line(),
                message: format!("expected '{{' after union name '{name}'"),
            }),
        }

        let mut members = Vec::new();

        loop {
            match self.peek() {
                None => break,
                Some(Token::RBrace) => { self.advance(); break; }
                Some(Token::Comma) | Some(Token::Semicolon) => { self.advance(); }
                Some(Token::Word(_)) => {
                    let member = self.expect_word()?;
                    members.push(member);
                    if matches!(self.peek(), Some(Token::Comma)) {
                        self.advance();
                    }
                }
                _ => { self.advance(); }
            }
        }

        Ok(UnionBlob { name, members, doc_comment })
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn parse_fbs(source: &str) -> Result<ParsedFile, ParseError> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenise()?;
    let mut parser = Parser::new(tokens);
    parser.parse_file()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_namespace_and_include() {
        let src = r#"
            namespace Payments.V1;
            include "common.fbs";
        "#;
        let f = parse_fbs(src).unwrap();
        assert_eq!(f.metadata.namespace, "Payments.V1");
        assert_eq!(f.metadata.imports, vec!["common.fbs"]);
    }

    #[test]
    fn parse_table_basic() {
        let src = r#"
            table Order {
                id: string;
                amount: int32;
            }
        "#;
        let f = parse_fbs(src).unwrap();
        assert_eq!(f.tables.len(), 1);
        assert_eq!(f.tables[0].name, "Order");
        assert_eq!(f.tables[0].fields.len(), 2);
        assert_eq!(f.tables[0].fields[0].name, "id");
        assert_eq!(f.tables[0].fields[0].field_type, "string");
        assert_eq!(f.tables[0].fields[0].slot_index, 0);
        assert_eq!(f.tables[0].fields[1].name, "amount");
        assert_eq!(f.tables[0].fields[1].slot_index, 1);
    }

    #[test]
    fn parse_table_vector_type() {
        let src = r#"
            table Order {
                items: [string];
            }
        "#;
        let f = parse_fbs(src).unwrap();
        assert_eq!(f.tables[0].fields[0].field_type, "[string]");
    }

    #[test]
    fn parse_table_default_value() {
        let src = r#"
            table Order {
                status: int32 = 0;
            }
        "#;
        let f = parse_fbs(src).unwrap();
        assert_eq!(f.tables[0].fields[0].default_value, "0");
    }

    #[test]
    fn parse_table_deprecated_attr() {
        let src = r#"
            table Order {
                old_field: string (deprecated);
            }
        "#;
        let f = parse_fbs(src).unwrap();
        assert!(f.tables[0].fields[0].deprecated);
    }

    #[test]
    fn parse_struct_basic() {
        let src = r#"
            struct Vec3 {
                x: float;
                y: float;
                z: float;
            }
        "#;
        let f = parse_fbs(src).unwrap();
        assert_eq!(f.structs.len(), 1);
        assert_eq!(f.structs[0].name, "Vec3");
        assert_eq!(f.structs[0].fields.len(), 3);
    }

    #[test]
    fn parse_enum_basic() {
        let src = r#"
            enum Color : byte {
                Red = 0,
                Green,
                Blue,
            }
        "#;
        let f = parse_fbs(src).unwrap();
        assert_eq!(f.enums.len(), 1);
        assert_eq!(f.enums[0].name, "Color");
        assert_eq!(f.enums[0].base_type, "byte");
        assert_eq!(f.enums[0].values.len(), 3);
        assert_eq!(f.enums[0].values[0].name, "Red");
        assert_eq!(f.enums[0].values[0].value, 0);
        assert_eq!(f.enums[0].values[1].name, "Green");
        assert_eq!(f.enums[0].values[1].value, 1);
        assert_eq!(f.enums[0].values[2].name, "Blue");
        assert_eq!(f.enums[0].values[2].value, 2);
    }

    #[test]
    fn parse_union_basic() {
        let src = r#"
            union Shape { Circle, Square, Triangle }
        "#;
        let f = parse_fbs(src).unwrap();
        assert_eq!(f.unions.len(), 1);
        assert_eq!(f.unions[0].name, "Shape");
        assert_eq!(f.unions[0].members, vec!["Circle", "Square", "Triangle"]);
    }

    #[test]
    fn parse_root_type_and_file_identifier() {
        let src = r#"
            table Order { id: string; }
            root_type Order;
            file_identifier "ORDR";
        "#;
        let f = parse_fbs(src).unwrap();
        assert_eq!(f.metadata.root_type, "Order");
        assert_eq!(f.metadata.file_identifier, vec!["ORDR"]);
    }

    #[test]
    fn parse_doc_comments() {
        let src = r#"
            // An order record
            table Order {
                // The unique order ID
                id: string;
            }
        "#;
        let f = parse_fbs(src).unwrap();
        assert!(f.tables[0].doc_comment.contains("order record"));
        assert!(f.tables[0].fields[0].doc_comment.contains("unique order ID"));
    }
}
