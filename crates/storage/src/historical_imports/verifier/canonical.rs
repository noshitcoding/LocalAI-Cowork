use super::{Error, HistoricalCell, MAX_CELL, MAX_DEPTH, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::fmt;

struct Strict(usize);
impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Value, D::Error> {
        if self.0 > MAX_DEPTH {
            return Err(serde::de::Error::custom("JSON depth limit"));
        }
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Strict {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded unique-key JSON")
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite JSON"))
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> std::result::Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut result = Vec::new();
        while let Some(value) = seq.next_element_seed(Strict(self.0 + 1))? {
            result.push(value);
        }
        Ok(Value::Array(result))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut result = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if result.contains_key(&key) {
                return Err(serde::de::Error::custom("duplicate JSON key"));
            }
            result.insert(key, map.next_value_seed(Strict(self.0 + 1))?);
        }
        Ok(Value::Object(result))
    }
}

pub(super) fn parse(raw: &[u8], maximum: usize) -> Result<Value> {
    if raw.len() > maximum {
        return Err(Error::Refused("JSON byte limit"));
    }
    let mut decoder = serde_json::Deserializer::from_slice(raw);
    let value = Strict(0)
        .deserialize(&mut decoder)
        .map_err(|_| Error::Refused("Invalid/duplicate/deep JSON"))?;
    decoder
        .end()
        .map_err(|_| Error::Refused("Trailing JSON bytes"))?;
    Ok(value)
}

pub(super) fn object<'a>(value: &'a Value, keys: &[&str]) -> Result<&'a Map<String, Value>> {
    let map = value
        .as_object()
        .ok_or(Error::Refused("JSON object required"))?;
    if map.len() != keys.len() || keys.iter().any(|key| !map.contains_key(*key)) {
        return Err(Error::Refused("Unknown/missing JSON fields"));
    }
    Ok(map)
}
pub(super) fn string(value: &Value) -> Result<&str> {
    value.as_str().ok_or(Error::Refused("String required"))
}
pub(super) fn natural(value: &Value, maximum: u64) -> Result<u64> {
    let n = value
        .as_u64()
        .ok_or(Error::Refused("Natural JSON integer required"))?;
    if n > maximum {
        return Err(Error::Refused("Integer limit"));
    }
    Ok(n)
}
pub(super) fn decimal(value: &str) -> Result<i64> {
    let n = value
        .parse::<i64>()
        .map_err(|_| Error::Refused("Historical int64 decimal required"))?;
    if n.to_string() != value {
        return Err(Error::Refused("Noncanonical integer decimal"));
    }
    Ok(n)
}
pub(super) fn identifier(value: &str) -> Result<&str> {
    if value.is_empty() || value.len() > 256 || value.chars().any(|c| c < ' ' || c == '\u{7f}') {
        return Err(Error::Refused(
            "Exact bounded historical identifier required",
        ));
    }
    Ok(value)
}
pub(super) fn hash(raw: &[u8]) -> [u8; 32] {
    Sha256::digest(raw).into()
}
pub(super) fn hex(raw: &[u8]) -> String {
    raw.iter().map(|b| format!("{b:02x}")).collect()
}
pub(super) fn hash_value(value: &Value) -> Result<String> {
    Ok(hex(&hash(&canonical(value)?)))
}
pub(super) fn sha(value: &Value) -> Result<&str> {
    let value = string(value)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Refused("Lowercase SHA256 required"));
    }
    Ok(value)
}

pub(super) fn canonical(value: &Value) -> Result<Vec<u8>> {
    fn quoted(value: &str, output: &mut Vec<u8>) {
        output.push(b'"');
        for c in value.chars() {
            match c {
                '"' => output.extend(b"\\\""),
                '\\' => output.extend(b"\\\\"),
                '\u{8}' => output.extend(b"\\b"),
                '\u{c}' => output.extend(b"\\f"),
                '\n' => output.extend(b"\\n"),
                '\r' => output.extend(b"\\r"),
                '\t' => output.extend(b"\\t"),
                c if (' '..='\u{7e}').contains(&c) => output.push(c as u8),
                c => {
                    let mut units = [0; 2];
                    for unit in c.encode_utf16(&mut units) {
                        output.extend(format!("\\u{unit:04x}").as_bytes());
                    }
                }
            }
        }
        output.push(b'"');
    }
    fn emit(value: &Value, output: &mut Vec<u8>) -> Result<()> {
        match value {
            Value::Null => output.extend(b"null"),
            Value::Bool(v) => output.extend(if *v { &b"true"[..] } else { &b"false"[..] }),
            Value::Number(v) => {
                if !v.is_i64() && !v.is_u64() {
                    return Err(Error::Refused("Structural JSON real unsupported"));
                }
                output.extend(v.to_string().as_bytes());
            }
            Value::String(v) => quoted(v, output),
            Value::Array(values) => {
                output.push(b'[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(b',');
                    }
                    emit(value, output)?;
                }
                output.push(b']');
            }
            Value::Object(values) => {
                output.push(b'{');
                let mut entries: Vec<_> = values.iter().collect();
                entries.sort_by_key(|(key, _)| *key);
                for (index, (key, value)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        output.push(b',');
                    }
                    quoted(key, output);
                    output.push(b':');
                    emit(value, output)?;
                }
                output.push(b'}');
            }
        }
        Ok(())
    }
    let mut result = Vec::new();
    emit(value, &mut result)?;
    Ok(result)
}

pub(super) fn sealed(value: &Value) -> Result<()> {
    let mut copy = value
        .as_object()
        .ok_or(Error::Refused("Sealed object required"))?
        .clone();
    let seal = copy
        .remove("manifest_sha256")
        .ok_or(Error::Refused("Manifest seal missing"))?;
    if sha(&seal)? != hash_value(&Value::Object(copy))? {
        return Err(Error::Refused("Manifest content seal mismatch"));
    }
    Ok(())
}

pub(super) fn cell(value: &Value) -> Result<HistoricalCell> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or(Error::Refused("Typed cell required"))?;
    match kind {
        "null" => {
            object(value, &["type"])?;
            Ok(HistoricalCell::Null)
        }
        "integer" => {
            object(value, &["type", "decimal"])?;
            Ok(HistoricalCell::Integer(decimal(string(
                &value["decimal"],
            )?)?))
        }
        "real" => {
            object(value, &["type", "ieee754_binary64_be"])?;
            let encoded = string(&value["ieee754_binary64_be"])?;
            if encoded.len() != 16
                || !encoded
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Error::Refused("Real bit encoding"));
            }
            let bits = u64::from_str_radix(encoded, 16).map_err(|_| Error::Refused("Real bits"))?;
            if !f64::from_bits(bits).is_finite() {
                return Err(Error::Refused("Nonfinite historical real"));
            }
            Ok(HistoricalCell::RealBits(bits))
        }
        "text" | "blob" => {
            object(value, &["type", "base64"])?;
            let encoded = string(&value["base64"])?;
            if encoded.len() > MAX_CELL.div_ceil(3) * 4 {
                return Err(Error::Refused("Cell byte limit"));
            }
            let raw = STANDARD
                .decode(encoded)
                .map_err(|_| Error::Refused("Invalid base64"))?;
            if raw.len() > MAX_CELL || STANDARD.encode(&raw) != encoded {
                return Err(Error::Refused("Noncanonical/oversized base64"));
            }
            Ok(if kind == "text" {
                HistoricalCell::TextBytes(raw)
            } else {
                HistoricalCell::Blob(raw)
            })
        }
        _ => Err(Error::Refused("Unknown historical storage class")),
    }
}

pub(super) fn encoded_cell(cell: &HistoricalCell) -> Value {
    match cell {
        HistoricalCell::Null => serde_json::json!({"type":"null"}),
        HistoricalCell::Integer(n) => serde_json::json!({"type":"integer","decimal":n.to_string()}),
        HistoricalCell::RealBits(bits) => {
            serde_json::json!({"type":"real","ieee754_binary64_be":format!("{bits:016x}")})
        }
        HistoricalCell::TextBytes(raw) => {
            serde_json::json!({"type":"text","base64":STANDARD.encode(raw)})
        }
        HistoricalCell::Blob(raw) => {
            serde_json::json!({"type":"blob","base64":STANDARD.encode(raw)})
        }
    }
}
