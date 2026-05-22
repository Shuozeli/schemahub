use schemahub_types::errors::ParseError;

use crate::ast::{EnumBlob, EnumValueDef, FieldDef, MessageBlob, ReservedRange, RpcDef, ServiceBlob};

// ── Public output ─────────────────────────────────────────────────────────────

pub struct ProtoFile {
    pub syntax: String,
    pub package: String,
    pub imports: Vec<String>,
    pub messages: Vec<MessageBlob>,
    pub enums: Vec<EnumBlob>,
    pub services: Vec<ServiceBlob>,
}

// ── Tokeniser ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    StringLit(String),
    Eq,
    Semicolon,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Comma,
    /// A line comment (content after `//`)
    LineComment(String),
    Slash,
    Dash,
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
            if ch.is_ascii_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn read_word(&mut self, first: u8) -> String {
        let mut word = vec![first];
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'.' {
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
                    // consume next char as-is
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
        // We've just consumed `/*`. Find closing `*/`.
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
                Some(b'=') => { self.advance(); tokens.push((Token::Eq, line)); }
                Some(b';') => { self.advance(); tokens.push((Token::Semicolon, line)); }
                Some(b'{') => { self.advance(); tokens.push((Token::LBrace, line)); }
                Some(b'}') => { self.advance(); tokens.push((Token::RBrace, line)); }
                Some(b'(') => { self.advance(); tokens.push((Token::LParen, line)); }
                Some(b')') => { self.advance(); tokens.push((Token::RParen, line)); }
                Some(b',') => { self.advance(); tokens.push((Token::Comma, line)); }
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
                        _ => {
                            tokens.push((Token::Slash, line));
                        }
                    }
                }
                Some(ch) if ch.is_ascii_alphanumeric() || ch == b'_' => {
                    let c = self.advance().unwrap();
                    let w = self.read_word(c);
                    tokens.push((Token::Word(w), line));
                }
                Some(other) => {
                    // skip unknown characters
                    self.advance();
                    let _ = other;
                }
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
            _ => Ok(()), // lenient — skip missing semicolons
        }
    }

    /// Collect any consecutive line comments as a doc comment string.
    fn collect_pending_comments(&mut self) -> String {
        let mut parts = Vec::new();
        loop {
            let is_comment = matches!(self.peek(), Some(Token::LineComment(_)));
            if !is_comment {
                break;
            }
            let tok = self.advance();
            if let Some(Token::LineComment(c)) = tok {
                let trimmed = c.trim_start_matches('/').trim().to_owned();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_owned());
                }
            }
        }
        parts.join("\n")
    }

    /// Skip tokens until we've consumed a matching `}` for the `{` already consumed.
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

    /// Skip to (and including) the next semicolon at the top level.
    fn skip_to_semicolon(&mut self) {
        while let Some(tok) = self.advance() {
            match tok {
                Token::Semicolon => break,
                Token::LBrace => { self.skip_block(); }
                _ => {}
            }
        }
    }

    /// Parse a dotted name that may contain dots (for qualified type names).
    fn parse_type_name(&mut self) -> Result<String, ParseError> {
        // After peeking, the lexer already joined dots into words (read_word handles '.'),
        // so a single Word token may already contain dots.
        let line = self.peek_line();
        match self.advance() {
            Some(Token::Word(w)) => Ok(w.clone()),
            other => Err(ParseError::SyntaxError {
                line,
                message: format!("expected type name, got {:?}", other),
            }),
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

    fn parse_file(&mut self) -> Result<ProtoFile, ParseError> {
        let mut file = ProtoFile {
            syntax: "proto3".into(),
            package: String::new(),
            imports: Vec::new(),
            messages: Vec::new(),
            enums: Vec::new(),
            services: Vec::new(),
        };

        // Collect nested declarations hoisted from inside message bodies.
        let mut hoisted_messages: Vec<MessageBlob> = Vec::new();
        let mut hoisted_enums: Vec<EnumBlob> = Vec::new();

        while self.peek().is_some() {
            let doc = self.collect_pending_comments();
            match self.peek() {
                None => break,
                Some(Token::Word(w)) => match w.as_str() {
                    "syntax" => {
                        self.advance();
                        if let Some(Token::Eq) = self.advance() {}
                        match self.advance() {
                            Some(Token::StringLit(s)) => file.syntax = s.clone(),
                            _ => {}
                        }
                        self.expect_semicolon()?;
                    }
                    "package" => {
                        self.advance();
                        file.package = self.expect_word()?;
                        self.expect_semicolon()?;
                    }
                    "import" => {
                        self.advance();
                        if matches!(self.peek(), Some(Token::Word(w)) if w == "public" || w == "weak") {
                            self.advance();
                        }
                        match self.advance() {
                            Some(Token::StringLit(s)) => file.imports.push(s.clone()),
                            _ => {}
                        }
                        self.expect_semicolon()?;
                    }
                    "message" => {
                        self.advance();
                        let msg = self.parse_message(doc, &mut hoisted_messages, &mut hoisted_enums)?;
                        file.messages.push(msg);
                    }
                    "enum" => {
                        self.advance();
                        let e = self.parse_enum(doc)?;
                        file.enums.push(e);
                    }
                    "service" => {
                        self.advance();
                        let svc = self.parse_service(doc)?;
                        file.services.push(svc);
                    }
                    "option" => {
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

        // Append hoisted nested declarations after all top-level ones.
        file.messages.extend(hoisted_messages);
        file.enums.extend(hoisted_enums);

        Ok(file)
    }

    // ── Message ───────────────────────────────────────────────────────────────

    fn parse_message(
        &mut self,
        doc_comment: String,
        hoisted_messages: &mut Vec<MessageBlob>,
        hoisted_enums: &mut Vec<EnumBlob>,
    ) -> Result<MessageBlob, ParseError> {
        let name = self.expect_word()?;
        match self.advance() {
            Some(Token::LBrace) => {}
            _ => {
                return Err(ParseError::SyntaxError {
                    line: self.peek_line(),
                    message: format!("expected '{{' after message name '{name}'"),
                });
            }
        }

        let mut fields = Vec::new();
        let mut reserved_numbers = Vec::new();
        let mut reserved_names = Vec::new();

        loop {
            let field_doc = self.collect_pending_comments();
            match self.peek() {
                None => break,
                Some(Token::RBrace) => { self.advance(); break; }
                Some(Token::Semicolon) => { self.advance(); continue; }
                Some(Token::Word(w)) => match w.as_str() {
                    "reserved" => {
                        self.advance();
                        self.parse_reserved(&mut reserved_numbers, &mut reserved_names)?;
                    }
                    "option" => {
                        self.skip_to_semicolon();
                    }
                    "message" => {
                        // Nested message: parse recursively and hoist with qualified name.
                        self.advance();
                        let mut nested = self.parse_message(field_doc, hoisted_messages, hoisted_enums)?;
                        nested.name = format!("{}.{}", name, nested.name);
                        hoisted_messages.push(nested);
                    }
                    "enum" => {
                        // Nested enum: parse and hoist with qualified name.
                        self.advance();
                        let mut nested = self.parse_enum(field_doc)?;
                        nested.name = format!("{}.{}", name, nested.name);
                        hoisted_enums.push(nested);
                    }
                    "oneof" => {
                        // Parse the oneof block; each field carries `oneof_name` for round-trip fidelity.
                        self.advance();
                        let oneof_name = match self.expect_word() {
                            Ok(n) => n,
                            Err(_) => { self.skip_to_semicolon(); continue; }
                        };
                        if !matches!(self.peek(), Some(Token::LBrace)) {
                            self.skip_to_semicolon();
                            continue;
                        }
                        self.advance(); // consume '{'
                        loop {
                            let fdoc = self.collect_pending_comments();
                            match self.peek() {
                                None => break,
                                Some(Token::RBrace) => { self.advance(); break; }
                                Some(Token::Semicolon) => { self.advance(); continue; }
                                Some(Token::Word(w)) if w == "option" => {
                                    self.skip_to_semicolon();
                                }
                                Some(Token::Word(_)) => {
                                    if let Some(mut f) = self.parse_field(fdoc)? {
                                        f.oneof_name = oneof_name.clone();
                                        fields.push(f);
                                    }
                                }
                                _ => { self.advance(); }
                            }
                        }
                    }
                    "map" => {
                        // map<KeyType, ValueType> field_name = N;
                        // The lexer drops '<' and '>' as unknown chars, so the token stream is:
                        //   Word("map") Word(key) Comma Word(value) Word(field_name) Eq Word(N) ...
                        self.advance(); // consume "map"
                        let key_type = match self.peek() {
                            Some(Token::Word(_)) => self.expect_word()?,
                            _ => { self.skip_to_semicolon(); continue; }
                        };
                        if matches!(self.peek(), Some(Token::Comma)) { self.advance(); }
                        let value_type = match self.peek() {
                            Some(Token::Word(_)) => self.expect_word()?,
                            _ => { self.skip_to_semicolon(); continue; }
                        };
                        let field_name = match self.peek() {
                            Some(Token::Word(_)) => self.expect_word()?,
                            _ => { self.skip_to_semicolon(); continue; }
                        };
                        if !matches!(self.advance(), Some(Token::Eq)) {
                            self.skip_to_semicolon();
                            continue;
                        }
                        let number = match self.parse_number() {
                            Ok(n) => n as u32,
                            Err(_) => { self.skip_to_semicolon(); continue; }
                        };
                        self.skip_to_semicolon();
                        fields.push(FieldDef {
                            name: field_name,
                            field_type: value_type,
                            map_key_type: key_type,
                            number,
                            doc_comment: field_doc,
                            ..Default::default()
                        });
                    }
                    _ => {
                        // Regular or repeated field
                        if let Some(field) = self.parse_field(field_doc)? {
                            fields.push(field);
                        }
                    }
                },
                _ => { self.advance(); }
            }
        }

        Ok(MessageBlob {
            blob_version: 1,
            name,
            fields,
            reserved_numbers,
            reserved_names,
            doc_comment,
        })
    }

    fn parse_field(&mut self, doc_comment: String) -> Result<Option<FieldDef>, ParseError> {
        // current token is at start of field declaration (type or "repeated")
        let repeated;
        let field_type;
        match self.peek() {
            Some(Token::Word(w)) if w == "repeated" => {
                self.advance();
                repeated = true;
                field_type = self.parse_type_name()?;
            }
            Some(Token::Word(_)) => {
                repeated = false;
                field_type = self.parse_type_name()?;
            }
            _ => {
                self.advance();
                return Ok(None);
            }
        }

        // field_name
        let name = match self.advance() {
            Some(Token::Word(w)) => w.clone(),
            _ => return Ok(None),
        };

        // = number
        match self.advance() {
            Some(Token::Eq) => {}
            _ => {
                // not a field — skip to semicolon
                self.skip_to_semicolon();
                return Ok(None);
            }
        }

        let number = self.parse_number()? as u32;

        // optional [options] and semicolon
        // skip any [ ... ] block options
        if let Some(Token::Word(_)) = self.peek() {
            // something unexpected; skip to semicolon
            self.skip_to_semicolon();
        } else {
            self.skip_to_semicolon();
        }

        Ok(Some(FieldDef {
            name,
            field_type,
            number,
            repeated,
            doc_comment,
            ..Default::default()
        }))
    }

    fn parse_reserved(
        &mut self,
        numbers: &mut Vec<ReservedRange>,
        names: &mut Vec<String>,
    ) -> Result<(), ParseError> {
        loop {
            match self.peek() {
                Some(Token::Semicolon) => { self.advance(); break; }
                Some(Token::Comma) => { self.advance(); continue; }
                Some(Token::StringLit(_)) => {
                    if let Some(Token::StringLit(s)) = self.advance() {
                        names.push(s.clone());
                    }
                }
                Some(Token::Word(_)) | Some(Token::Dash) => {
                    // number or range
                    let start = self.parse_number()? as u32;
                    if matches!(self.peek(), Some(Token::Word(w)) if w == "to") {
                        self.advance();
                        let end = if matches!(self.peek(), Some(Token::Word(w)) if w == "max") {
                            self.advance();
                            u32::MAX
                        } else {
                            self.parse_number()? as u32
                        };
                        numbers.push(ReservedRange { start, end });
                    } else {
                        numbers.push(ReservedRange { start, end: start });
                    }
                }
                _ => { self.advance(); break; }
            }
        }
        Ok(())
    }

    // ── Enum ─────────────────────────────────────────────────────────────────

    fn parse_enum(&mut self, doc_comment: String) -> Result<EnumBlob, ParseError> {
        let name = self.expect_word()?;
        match self.advance() {
            Some(Token::LBrace) => {}
            _ => return Err(ParseError::SyntaxError {
                line: self.peek_line(),
                message: format!("expected '{{' after enum name '{name}'"),
            }),
        }

        let mut values = Vec::new();

        loop {
            let vdoc = self.collect_pending_comments();
            match self.peek() {
                None => break,
                Some(Token::RBrace) => { self.advance(); break; }
                Some(Token::Semicolon) => { self.advance(); continue; }
                Some(Token::Word(w)) if w == "option" => {
                    self.skip_to_semicolon();
                }
                Some(Token::Word(_)) => {
                    let value_name = self.expect_word()?;
                    // = number;
                    match self.advance() {
                        Some(Token::Eq) => {}
                        _ => { self.skip_to_semicolon(); continue; }
                    }
                    let number = self.parse_number()? as i32;
                    // skip options [...]
                    self.skip_to_semicolon();
                    values.push(EnumValueDef {
                        name: value_name,
                        number,
                        doc_comment: vdoc,
                    });
                }
                _ => { self.advance(); }
            }
        }

        Ok(EnumBlob {
            blob_version: 1,
            name,
            values,
            doc_comment,
        })
    }

    // ── Service ───────────────────────────────────────────────────────────────

    fn parse_service(&mut self, doc_comment: String) -> Result<ServiceBlob, ParseError> {
        let name = self.expect_word()?;
        match self.advance() {
            Some(Token::LBrace) => {}
            _ => return Err(ParseError::SyntaxError {
                line: self.peek_line(),
                message: format!("expected '{{' after service name '{name}'"),
            }),
        }

        let mut rpcs = Vec::new();

        loop {
            let rdoc = self.collect_pending_comments();
            match self.peek() {
                None => break,
                Some(Token::RBrace) => { self.advance(); break; }
                Some(Token::Semicolon) => { self.advance(); continue; }
                Some(Token::Word(w)) if w == "rpc" => {
                    self.advance();
                    if let Some(rpc) = self.parse_rpc(rdoc)? {
                        rpcs.push(rpc);
                    }
                }
                Some(Token::Word(w)) if w == "option" => {
                    self.skip_to_semicolon();
                }
                _ => { self.advance(); }
            }
        }

        Ok(ServiceBlob {
            blob_version: 1,
            name,
            rpcs,
            doc_comment,
        })
    }

    fn parse_rpc(&mut self, doc_comment: String) -> Result<Option<RpcDef>, ParseError> {
        let rpc_name = self.expect_word()?;

        // (stream? RequestType)
        match self.advance() {
            Some(Token::LParen) => {}
            _ => { self.skip_to_semicolon(); return Ok(None); }
        }
        let client_streaming = if matches!(self.peek(), Some(Token::Word(w)) if w == "stream") {
            self.advance();
            true
        } else {
            false
        };
        let request_type = self.parse_type_name()?;
        match self.advance() {
            Some(Token::RParen) => {}
            _ => { self.skip_to_semicolon(); return Ok(None); }
        }

        // returns
        match self.advance() {
            Some(Token::Word(w)) if w == "returns" => {}
            _ => { self.skip_to_semicolon(); return Ok(None); }
        }

        // (stream? ResponseType)
        match self.advance() {
            Some(Token::LParen) => {}
            _ => { self.skip_to_semicolon(); return Ok(None); }
        }
        let server_streaming = if matches!(self.peek(), Some(Token::Word(w)) if w == "stream") {
            self.advance();
            true
        } else {
            false
        };
        let response_type = self.parse_type_name()?;
        match self.advance() {
            Some(Token::RParen) => {}
            _ => { self.skip_to_semicolon(); return Ok(None); }
        }

        // optional { ... } body with options, or just ;
        match self.peek() {
            Some(Token::LBrace) => { self.advance(); self.skip_block(); }
            Some(Token::Semicolon) => { self.advance(); }
            _ => {}
        }

        Ok(Some(RpcDef {
            name: rpc_name,
            request_type,
            response_type,
            client_streaming,
            server_streaming,
            doc_comment,
        }))
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn parse_proto(source: &str) -> Result<ProtoFile, ParseError> {
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
    fn parse_simple_message() {
        let source = r#"
            syntax = "proto3";
            package payments;

            message CreatePaymentRequest {
              string user_id = 1;
              int64 amount_cents = 2;
            }
        "#;
        let result = parse_proto(source).unwrap();
        assert_eq!(result.package, "payments");
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].name, "CreatePaymentRequest");
        assert_eq!(result.messages[0].fields.len(), 2);
        assert_eq!(result.messages[0].fields[0].name, "user_id");
        assert_eq!(result.messages[0].fields[0].number, 1);
        assert_eq!(result.messages[0].fields[1].name, "amount_cents");
        assert_eq!(result.messages[0].fields[1].number, 2);
    }

    #[test]
    fn parse_enum() {
        let source = r#"
            syntax = "proto3";
            enum PaymentStatus {
              UNKNOWN = 0;
              PENDING = 1;
              COMPLETED = 2;
            }
        "#;
        let result = parse_proto(source).unwrap();
        assert_eq!(result.enums.len(), 1);
        assert_eq!(result.enums[0].name, "PaymentStatus");
        assert_eq!(result.enums[0].values.len(), 3);
        assert_eq!(result.enums[0].values[0].name, "UNKNOWN");
        assert_eq!(result.enums[0].values[0].number, 0);
    }

    #[test]
    fn parse_service() {
        let source = r#"
            syntax = "proto3";
            service PaymentService {
              rpc CreatePayment (CreatePaymentRequest) returns (CreatePaymentResponse);
              rpc StreamPayments (StreamRequest) returns (stream PaymentEvent);
            }
        "#;
        let result = parse_proto(source).unwrap();
        assert_eq!(result.services.len(), 1);
        let svc = &result.services[0];
        assert_eq!(svc.name, "PaymentService");
        assert_eq!(svc.rpcs.len(), 2);
        assert!(!svc.rpcs[0].server_streaming);
        assert!(svc.rpcs[1].server_streaming);
    }

    #[test]
    fn parse_repeated_field() {
        let source = r#"
            syntax = "proto3";
            message Order {
              repeated string item_ids = 1;
            }
        "#;
        let result = parse_proto(source).unwrap();
        assert!(result.messages[0].fields[0].repeated);
    }

    #[test]
    fn parse_reserved() {
        let source = r#"
            syntax = "proto3";
            message Foo {
              string name = 3;
              reserved 1, 2;
              reserved "old_field";
            }
        "#;
        let result = parse_proto(source).unwrap();
        let msg = &result.messages[0];
        assert_eq!(msg.reserved_numbers.len(), 2);
        assert_eq!(msg.reserved_names, vec!["old_field"]);
    }

    #[test]
    fn parse_imports() {
        let source = r#"
            syntax = "proto3";
            import "google/protobuf/timestamp.proto";
            import "other/file.proto";
            message Foo { string x = 1; }
        "#;
        let result = parse_proto(source).unwrap();
        assert_eq!(result.imports.len(), 2);
        assert_eq!(result.imports[0], "google/protobuf/timestamp.proto");
    }

    #[test]
    fn parse_doc_comments() {
        let source = r#"
            syntax = "proto3";
            // This is a payment request
            message CreatePaymentRequest {
              // The user identifier
              string user_id = 1;
            }
        "#;
        let result = parse_proto(source).unwrap();
        assert!(result.messages[0].doc_comment.contains("payment request"));
        assert!(result.messages[0].fields[0].doc_comment.contains("user identifier"));
    }

    #[test]
    fn parse_oneof_fields_extracted() {
        let source = r#"
            syntax = "proto3";
            message Event {
              string id = 1;
              oneof payload {
                string text_body = 2;
                bytes binary_body = 3;
              }
              bool archived = 4;
            }
        "#;
        let result = parse_proto(source).unwrap();
        assert_eq!(result.messages.len(), 1);
        let msg = &result.messages[0];
        assert_eq!(msg.fields.len(), 4);

        // Regular fields have empty oneof_name
        assert_eq!(msg.fields[0].name, "id");
        assert!(msg.fields[0].oneof_name.is_empty());
        assert_eq!(msg.fields[3].name, "archived");
        assert!(msg.fields[3].oneof_name.is_empty());

        // Oneof fields carry the group name
        assert_eq!(msg.fields[1].name, "text_body");
        assert_eq!(msg.fields[1].oneof_name, "payload");
        assert_eq!(msg.fields[2].name, "binary_body");
        assert_eq!(msg.fields[2].oneof_name, "payload");
    }

    #[test]
    fn parse_map_field() {
        let source = r#"
            syntax = "proto3";
            message Config {
              map<string, string> labels = 1;
              map<string, int32> counts = 2;
            }
        "#;
        let result = parse_proto(source).unwrap();
        let msg = &result.messages[0];
        assert_eq!(msg.fields.len(), 2);

        assert_eq!(msg.fields[0].name, "labels");
        assert_eq!(msg.fields[0].map_key_type, "string");
        assert_eq!(msg.fields[0].field_type, "string");
        assert_eq!(msg.fields[0].number, 1);

        assert_eq!(msg.fields[1].name, "counts");
        assert_eq!(msg.fields[1].map_key_type, "string");
        assert_eq!(msg.fields[1].field_type, "int32");
        assert_eq!(msg.fields[1].number, 2);
    }

    #[test]
    fn parse_nested_message_hoisted() {
        let source = r#"
            syntax = "proto3";
            message Outer {
              string name = 1;
              message Inner {
                string id = 1;
              }
              Inner inner = 2;
            }
        "#;
        let result = parse_proto(source).unwrap();
        // Outer is the top-level message
        assert_eq!(result.messages.len(), 2);
        let outer = result.messages.iter().find(|m| m.name == "Outer").unwrap();
        assert_eq!(outer.fields.len(), 2);
        assert_eq!(outer.fields[0].name, "name");
        assert_eq!(outer.fields[1].name, "inner");

        // Nested Inner is hoisted as "Outer.Inner"
        let inner = result.messages.iter().find(|m| m.name == "Outer.Inner").unwrap();
        assert_eq!(inner.fields.len(), 1);
        assert_eq!(inner.fields[0].name, "id");
    }

    #[test]
    fn parse_nested_enum_hoisted() {
        let source = r#"
            syntax = "proto3";
            message Order {
              enum Status {
                UNKNOWN = 0;
                PLACED = 1;
              }
              Status status = 1;
            }
        "#;
        let result = parse_proto(source).unwrap();
        assert_eq!(result.messages.len(), 1);
        // Nested enum hoisted as "Order.Status"
        assert_eq!(result.enums.len(), 1);
        assert_eq!(result.enums[0].name, "Order.Status");
        assert_eq!(result.enums[0].values.len(), 2);
    }
}
