use crate::{Error, Result};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::fmt;

/// Reject duplicate keys rather than interpreting an ambiguous JSON-RPC envelope
/// or schema differently from the remote server.
pub(crate) fn parse(bytes: &[u8]) -> Result<Value> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValue::deserialize(&mut de)
        .map_err(|_| Error::Protocol)?
        .0;
    de.end().map_err(|_| Error::Protocol)?;
    Ok(value)
}
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StrictValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unambiguous JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(v.into())))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(v.into())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                Number::from_f64(v)
                    .map(|n| StrictValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::String(v)))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut access: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut result = Vec::new();
                while let Some(v) = access.next_element::<StrictValue>()? {
                    result.push(v.0);
                }
                Ok(StrictValue(Value::Array(result)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut access: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut result = Map::new();
                while let Some(k) = access.next_key::<String>()? {
                    if result.contains_key(&k) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    result.insert(k, access.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(result)))
            }
        }
        de.deserialize_any(V)
    }
}

/// Bounded incremental SSE parser. Reconnect IDs/retry hints never trigger I/O.
pub(crate) struct Sse {
    line: Vec<u8>,
    data: Vec<u8>,
    first_line: bool,
    after_cr: bool,
    limit: usize,
    message_limit: usize,
}
impl Sse {
    pub fn new(limit: usize, message_limit: usize) -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            first_line: true,
            after_cr: false,
            limit,
            message_limit,
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>> {
        let mut messages = Vec::new();
        for &byte in bytes {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' || byte == b'\n' {
                self.after_cr = byte == b'\r';
                if let Some(value) = self.end_line()? {
                    if messages.len() >= self.message_limit {
                        return Err(Error::Limit);
                    }
                    messages.push(value);
                }
            } else {
                if self.line.len() + self.data.len() >= self.limit {
                    return Err(Error::Limit);
                }
                self.line.push(byte);
            }
        }
        Ok(messages)
    }
    fn end_line(&mut self) -> Result<Option<Value>> {
        let bytes = std::mem::take(&mut self.line);
        let mut line = std::str::from_utf8(&bytes).map_err(|_| Error::Protocol)?;
        if self.first_line {
            self.first_line = false;
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
        }
        if line.is_empty() {
            if self.data.is_empty() {
                return Ok(None);
            }
            self.data.pop(); // last appended LF
            // MCP 2025-11-25 permits an empty SSE priming event. It is not a
            // JSON-RPC message, response acknowledgement, or permission to replay.
            if self.data.is_empty() {
                return Ok(None);
            }
            let value = parse(&self.data)?;
            self.data.clear();
            return Ok(Some(value));
        }
        let (key, value) = line.split_once(':').unwrap_or((line, ""));
        if key == "data" {
            let value = value.strip_prefix(' ').unwrap_or(value);
            if self.data.len() + value.len() + 1 > self.limit {
                return Err(Error::Limit);
            }
            self.data.extend_from_slice(value.as_bytes());
            self.data.push(b'\n');
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sse_split_utf8_crlf_bom_comments_and_multiline() {
        let input =
            "\u{feff}: ping\r\nid: opaque\rretry: 1\rdata: {\"text\":\r\ndata: \"grün\"}\r\n\r\n";
        let mut parser = Sse::new(1024, 8);
        let mut values = Vec::new();
        for byte in input.as_bytes() {
            values.extend(parser.push(&[*byte]).unwrap());
        }
        assert_eq!(values, vec![serde_json::json!({"text":"grün"})]);
    }
    #[test]
    fn ambiguous_json_and_unterminated_sse_are_not_accepted() {
        assert_eq!(parse(br#"{"result":{},"result":{}}"#), Err(Error::Protocol));
        assert_eq!(parse(b"{} {}"), Err(Error::Protocol));
        let mut parser = Sse::new(32, 8);
        assert!(parser.push(b"data: {}\n").unwrap().is_empty());
        assert_eq!(parser.push(&[b'x'; 33]), Err(Error::Limit));
    }
    #[test]
    fn a_single_network_chunk_cannot_allocate_an_unbounded_message_vector() {
        let mut parser = Sse::new(4096, 2);
        assert_eq!(
            parser.push(b"data: {}\n\ndata: {}\n\ndata: {}\n\n"),
            Err(Error::Limit)
        );
    }
    #[test]
    fn empty_priming_event_is_ignored_without_acknowledging_a_request() {
        let mut parser = Sse::new(4096, 4);
        assert!(
            parser
                .push(b"id: initial\nretry: 1000\ndata:\n\n")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            parser
                .push(b"data: {\"jsonrpc\":\"2.0\",\"id\":\"a\",\"result\":{}}\n\n")
                .unwrap()
                .len(),
            1
        );
    }
}
