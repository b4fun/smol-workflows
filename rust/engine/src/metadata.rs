use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct WorkflowMetadata {
    pub name: String,
    pub description: String,
    #[serde(rename = "whenToUse", skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<WorkflowPhaseMetadata>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct WorkflowPhaseMetadata {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

pub fn read_workflow_metadata(path: impl AsRef<Path>) -> anyhow::Result<Option<WorkflowMetadata>> {
    let source = match fs::read_to_string(path.as_ref()) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };

    Ok(extract_workflow_metadata(&source))
}

pub fn extract_workflow_metadata(source: &str) -> Option<WorkflowMetadata> {
    let mut parser = Parser::new(source);
    let value = parser.find_exported_const_meta()?;
    to_workflow_metadata(value)
}

fn to_workflow_metadata(value: serde_json::Value) -> Option<WorkflowMetadata> {
    let object = value.as_object()?;
    let name = object.get("name")?.as_str()?.to_string();
    let description = object.get("description")?.as_str()?.to_string();
    let when_to_use = object
        .get("whenToUse")
        .and_then(|value| value.as_str())
        .map(ToString::to_string);
    let phases = object
        .get("phases")
        .and_then(|value| value.as_array())
        .map(|phases| {
            phases
                .iter()
                .filter_map(to_workflow_phase_metadata)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(WorkflowMetadata {
        name,
        description,
        when_to_use,
        phases,
    })
}

fn to_workflow_phase_metadata(value: &serde_json::Value) -> Option<WorkflowPhaseMetadata> {
    let object = value.as_object()?;
    Some(WorkflowPhaseMetadata {
        title: object.get("title")?.as_str()?.to_string(),
        detail: object
            .get("detail")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        model: object
            .get("model")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        provider: object
            .get("provider")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
    })
}

struct Parser<'a> {
    source: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: 0,
        }
    }

    fn find_exported_const_meta(&mut self) -> Option<serde_json::Value> {
        while self.skip_ws_comments() {
            let checkpoint = self.pos;
            if self.consume_keyword("export")
                && self.skip_ws_comments()
                && self.consume_keyword("const")
                && self.skip_ws_comments()
                && self.consume_keyword("meta")
                && self.skip_ws_comments()
                && self.consume_byte(b'=')
            {
                self.skip_ws_comments();
                let value = self.parse_value().ok()?;
                return Some(value);
            }
            self.pos = checkpoint;
            self.skip_one_token();
        }
        None
    }

    fn parse_value(&mut self) -> anyhow::Result<serde_json::Value> {
        self.skip_ws_comments();
        match self.peek_byte() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'\'') | Some(b'"') => self.parse_string().map(serde_json::Value::String),
            Some(b'-') | Some(b'+') | Some(b'0'..=b'9') => {
                self.parse_number().map(serde_json::Value::from)
            }
            Some(_) if self.consume_keyword("true") => Ok(serde_json::Value::Bool(true)),
            Some(_) if self.consume_keyword("false") => Ok(serde_json::Value::Bool(false)),
            Some(_) if self.consume_keyword("null") => Ok(serde_json::Value::Null),
            _ => anyhow::bail!("unsupported metadata literal"),
        }
    }

    fn parse_object(&mut self) -> anyhow::Result<serde_json::Value> {
        self.expect_byte(b'{')?;
        let mut object = serde_json::Map::new();
        loop {
            self.skip_ws_comments();
            if self.consume_byte(b'}') {
                break;
            }
            if self.starts_with("...") || self.peek_byte() == Some(b'[') {
                anyhow::bail!("unsupported object property");
            }
            let key = self.parse_property_key()?;
            self.skip_ws_comments();
            self.expect_byte(b':')?;
            let value = self.parse_value()?;
            object.insert(key, value);
            self.skip_ws_comments();
            if self.consume_byte(b'}') {
                break;
            }
            self.expect_byte(b',')?;
        }
        Ok(serde_json::Value::Object(object))
    }

    fn parse_array(&mut self) -> anyhow::Result<serde_json::Value> {
        self.expect_byte(b'[')?;
        let mut array = Vec::new();
        loop {
            self.skip_ws_comments();
            if self.consume_byte(b']') {
                break;
            }
            if self.starts_with("...") || self.peek_byte() == Some(b',') {
                anyhow::bail!("unsupported array element");
            }
            array.push(self.parse_value()?);
            self.skip_ws_comments();
            if self.consume_byte(b']') {
                break;
            }
            self.expect_byte(b',')?;
        }
        Ok(serde_json::Value::Array(array))
    }

    fn parse_property_key(&mut self) -> anyhow::Result<String> {
        self.skip_ws_comments();
        match self.peek_byte() {
            Some(b'\'') | Some(b'"') => self.parse_string(),
            Some(b'0'..=b'9') => self.parse_number().map(|number| {
                if number.fract() == 0.0 {
                    format!("{}", number as i64)
                } else {
                    number.to_string()
                }
            }),
            Some(_) => self.parse_identifier(),
            None => anyhow::bail!("unexpected end of metadata"),
        }
    }

    fn parse_identifier(&mut self) -> anyhow::Result<String> {
        let start = self.pos;
        let Some(byte) = self.peek_byte() else {
            anyhow::bail!("unexpected end of metadata")
        };
        if !is_ident_start(byte) {
            anyhow::bail!("expected identifier")
        }
        self.pos += 1;
        while matches!(self.peek_byte(), Some(byte) if is_ident_continue(byte)) {
            self.pos += 1;
        }
        Ok(self.source[start..self.pos].to_string())
    }

    fn parse_string(&mut self) -> anyhow::Result<String> {
        let quote = self
            .peek_byte()
            .ok_or_else(|| anyhow::anyhow!("expected string"))?;
        if quote != b'\'' && quote != b'"' {
            anyhow::bail!("expected string")
        }
        self.pos += 1;
        let mut output = String::new();
        while let Some(byte) = self.peek_byte() {
            self.pos += 1;
            match byte {
                b if b == quote => return Ok(output),
                b'\\' => output.push(self.parse_escape()?),
                b => output.push(b as char),
            }
        }
        anyhow::bail!("unterminated string")
    }

    fn parse_escape(&mut self) -> anyhow::Result<char> {
        let byte = self
            .peek_byte()
            .ok_or_else(|| anyhow::anyhow!("unterminated escape"))?;
        self.pos += 1;
        Ok(match byte {
            b'"' => '"',
            b'\'' => '\'',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{0008}',
            b'f' => '\u{000c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let hex = self.take_chars(4)?;
                let value = u16::from_str_radix(hex, 16)?;
                char::from_u32(value as u32).ok_or_else(|| anyhow::anyhow!("invalid unicode"))?
            }
            b => b as char,
        })
    }

    fn parse_number(&mut self) -> anyhow::Result<f64> {
        let start = self.pos;
        if matches!(self.peek_byte(), Some(b'-' | b'+')) {
            self.pos += 1;
        }
        while matches!(self.peek_byte(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.consume_byte(b'.') {
            while matches!(self.peek_byte(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek_byte(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek_byte(), Some(b'-' | b'+')) {
                self.pos += 1;
            }
            while matches!(self.peek_byte(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = &self.source[start..self.pos];
        if text == "+" || text == "-" || text.is_empty() {
            anyhow::bail!("invalid number")
        }
        Ok(text.parse()?)
    }

    fn skip_ws_comments(&mut self) -> bool {
        loop {
            while matches!(self.peek_byte(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                self.pos += 1;
            }
            if self.starts_with("//") {
                while !matches!(self.peek_byte(), None | Some(b'\n')) {
                    self.pos += 1;
                }
                continue;
            }
            if self.starts_with("/*") {
                self.pos += 2;
                while self.pos + 1 < self.bytes.len() && !self.starts_with("*/") {
                    self.pos += 1;
                }
                self.pos = (self.pos + 2).min(self.bytes.len());
                continue;
            }
            return self.pos < self.bytes.len();
        }
    }

    fn skip_one_token(&mut self) {
        match self.peek_byte() {
            Some(b'\'' | b'"' | b'`') => self.skip_string_like(),
            Some(_) => self.pos += 1,
            None => {}
        }
    }

    fn skip_string_like(&mut self) {
        let Some(quote) = self.peek_byte() else {
            return;
        };
        self.pos += 1;
        while let Some(byte) = self.peek_byte() {
            self.pos += 1;
            if byte == b'\\' {
                self.pos = (self.pos + 1).min(self.bytes.len());
            } else if byte == quote {
                break;
            }
        }
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        if !self.starts_with(keyword) {
            return false;
        }
        let end = self.pos + keyword.len();
        if end < self.bytes.len() && is_ident_continue(self.bytes[end]) {
            return false;
        }
        if self.pos > 0 && is_ident_continue(self.bytes[self.pos - 1]) {
            return false;
        }
        self.pos = end;
        true
    }

    fn consume_byte(&mut self, byte: u8) -> bool {
        if self.peek_byte() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_byte(&mut self, byte: u8) -> anyhow::Result<()> {
        if self.consume_byte(byte) {
            Ok(())
        } else {
            anyhow::bail!("expected byte {}", byte as char)
        }
    }

    fn starts_with(&self, needle: &str) -> bool {
        self.source[self.pos..].starts_with(needle)
    }

    fn peek_byte(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn take_chars(&mut self, count: usize) -> anyhow::Result<&'a str> {
        let start = self.pos;
        let end = self.pos + count;
        if end > self.source.len() {
            anyhow::bail!("unexpected end")
        }
        self.pos = end;
        Ok(&self.source[start..end])
    }
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte == b'$' || byte.is_ascii_alphabetic()
}

fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}
