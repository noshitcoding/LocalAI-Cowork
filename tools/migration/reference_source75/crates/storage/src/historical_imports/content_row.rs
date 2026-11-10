//! Decode selected cells from a completely verified private row file, never a whole-row Value.
use super::{Fields, allowed};
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::{DeserializeSeed, Error as DeError, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::fmt;
const CELL_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn selected<R: std::io::Read>(
    input: R,
    table: &str,
    columns: &[String],
    rowid: i64,
) -> Result<(Fields, String)> {
    let mut decoder = serde_json::Deserializer::from_reader(input);
    let result = RowSeed {
        table,
        columns,
        rowid,
    }
    .deserialize(&mut decoder)
    .map_err(crate::storage_error)?;
    decoder.end().map_err(crate::storage_error)?;
    Ok(result)
}
struct RowSeed<'a> {
    table: &'a str,
    columns: &'a [String],
    rowid: i64,
}
impl<'de> DeserializeSeed<'de> for RowSeed<'_> {
    type Value = (Fields, String);
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for RowSeed<'_> {
    type Value = (Fields, String);
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("verified original row with exactly rowid and cells")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> std::result::Result<Self::Value, M::Error> {
        let mut rowid = None;
        let mut cells = None;
        while let Some(key) = m.next_key::<String>()? {
            match key.as_str() {
                "rowid" => {
                    if rowid.is_some() {
                        return Err(M::Error::custom("duplicate original rowid"));
                    }
                    let raw = m.next_value::<String>()?;
                    let n = raw
                        .parse::<i64>()
                        .map_err(|_| M::Error::custom("original rowid integer"))?;
                    if n.to_string() != raw {
                        return Err(M::Error::custom("noncanonical original rowid"));
                    }
                    rowid = Some(n);
                }
                "cells" => {
                    if cells.is_some() {
                        return Err(M::Error::custom("duplicate original cells"));
                    }
                    cells = Some(m.next_value_seed(CellsSeed {
                        table: self.table,
                        columns: self.columns,
                    })?);
                }
                _ => return Err(M::Error::custom("unexpected original row property")),
            }
        }
        if rowid != Some(self.rowid) {
            return Err(M::Error::custom(
                "captured original rowid differs from verified header",
            ));
        }
        cells.ok_or_else(|| M::Error::custom("missing original cells"))
    }
}
struct CellsSeed<'a> {
    table: &'a str,
    columns: &'a [String],
}
impl<'de> DeserializeSeed<'de> for CellsSeed<'_> {
    type Value = (Fields, String);
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for CellsSeed<'_> {
    type Value = (Fields, String);
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("exact original column-count typed cells")
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut s: S) -> std::result::Result<Self::Value, S::Error> {
        let mut fields = Fields::new();
        let mut unsupported = String::new();
        for name in self.columns {
            if allowed(self.table, name) {
                let cell = s
                    .next_element::<Value>()?
                    .ok_or_else(|| S::Error::custom("too few original cells"))?;
                let (value, reason) = cell_text(&cell)
                    .map_err(|_| S::Error::custom("original selected cell encoding differs"))?;
                if let Some(reason) = reason {
                    unsupported = reason.into();
                }
                if fields.insert(name.clone(), value).is_some() {
                    return Err(S::Error::custom("duplicate original selected column"));
                }
            } else {
                s.next_element::<IgnoredAny>()?
                    .ok_or_else(|| S::Error::custom("too few original cells"))?;
            }
        }
        if s.next_element::<IgnoredAny>()?.is_some() {
            return Err(S::Error::custom("too many original cells"));
        }
        Ok((fields, unsupported))
    }
}
fn cell_text(v: &Value) -> Result<(String, Option<&'static str>)> {
    let o = v.as_object().ok_or(Error::Conflict)?;
    match v["type"].as_str() {
        Some("null") if o.len() == 1 => Ok((String::new(), None)),
        Some("integer") if o.len() == 2 => {
            let raw = v["decimal"].as_str().ok_or(Error::Conflict)?;
            let n = raw.parse::<i64>().map_err(|_| Error::Conflict)?;
            if n.to_string() != raw {
                return Err(Error::Conflict);
            }
            Ok((raw.into(), None))
        }
        Some("text") if o.len() == 2 => {
            let encoded = v["base64"].as_str().ok_or(Error::Conflict)?;
            if encoded.len() > CELL_BYTES.div_ceil(3) * 4 {
                return Err(Error::BudgetExceeded);
            }
            let bytes = STANDARD.decode(encoded).map_err(|_| Error::Conflict)?;
            if bytes.len() > CELL_BYTES {
                return Err(Error::BudgetExceeded);
            }
            match String::from_utf8(bytes) {
                Ok(s) => Ok((s, None)),
                Err(_) => Ok((
                    String::new(),
                    Some(
                        "Original selected text is not UTF-8; complete bytes remain in quarantine",
                    ),
                )),
            }
        }
        Some("real" | "blob") => Ok((
            String::new(),
            Some(
                "Original selected cell has a non-renderable SQLite class; complete bytes remain in quarantine",
            ),
        )),
        _ => Err(Error::Conflict),
    }
}
