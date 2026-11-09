//! Private bounded derived-index staging. Original row bytes never become an operational model.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::fmt;

pub(super) const CELLS_PER_PAGE: usize = 16;
pub(super) const INDEX_FORMAT: &str = "localai-cowork-original-history-index/1";
pub(super) struct HistoryStage {
    pub file: std::fs::File,
    pub records: u64,
    pub reserved_bytes: u64,
    pub rows: u64,
    pub cells: u64,
    pub value_bytes: u64,
    pub table_pins: Vec<HistoricalHistoryPagePin>,
    digest: Sha256,
    maximum_reserved: u64,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StageRecord {
    pub stream: String,
    pub index: i64,
    pub size: u64,
    pub sha256: String,
}
fn private_file(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    options.open(path).map_err(storage_error)
}
impl HistoryStage {
    pub(super) fn new(root: &Path, maximum_reserved: u64) -> Result<Self> {
        Ok(Self {
            file: private_file(&root.join("history-index.stage"))?,
            records: 0,
            reserved_bytes: 0,
            rows: 0,
            cells: 0,
            value_bytes: 0,
            table_pins: vec![],
            digest: Sha256::new(),
            maximum_reserved,
        })
    }
    pub(super) fn row_file(root: &Path) -> Result<std::fs::File> {
        private_file(&root.join("history-row.stage"))
    }
    pub(super) fn put(
        &mut self,
        stream: &str,
        index: i64,
        bytes: &[u8],
    ) -> Result<HistoricalHistoryPagePin> {
        if bytes.len() > CHUNK_BYTES || index < 0 {
            return Err(Error::BudgetExceeded);
        }
        let charge = (bytes.len() as u64)
            .checked_mul(2)
            .and_then(|n| n.checked_add(4096))
            .ok_or(Error::BudgetExceeded)?;
        self.reserved_bytes = self
            .reserved_bytes
            .checked_add(charge)
            .ok_or(Error::BudgetExceeded)?;
        if self.reserved_bytes > self.maximum_reserved {
            return Err(Error::BudgetExceeded);
        }
        let pin = HistoricalHistoryPagePin {
            size: bytes.len() as u64,
            sha256: hash(bytes),
        };
        let header = json_string(&StageRecord {
            stream: stream.into(),
            index,
            size: pin.size,
            sha256: pin.sha256.clone(),
        })?;
        let length = u32::try_from(header.len())
            .map_err(|_| Error::BudgetExceeded)?
            .to_be_bytes();
        self.file.write_all(&length).map_err(storage_error)?;
        self.file
            .write_all(header.as_bytes())
            .map_err(storage_error)?;
        self.file.write_all(bytes).map_err(storage_error)?;
        self.digest.update(length);
        self.digest.update(header.as_bytes());
        self.digest.update(bytes);
        self.records += 1;
        Ok(pin)
    }
    pub(super) fn put_json<T: serde::Serialize>(
        &mut self,
        stream: &str,
        index: i64,
        value: &T,
    ) -> Result<HistoricalHistoryPagePin> {
        let raw = Zeroizing::new(serde_json::to_vec(value).map_err(storage_error)?);
        self.put(stream, index, &raw)
    }
    pub(super) fn digest(&self) -> String {
        format!("{:x}", self.digest.clone().finalize())
    }
    pub(super) fn rewind(&mut self) -> Result<()> {
        self.file.flush().map_err(storage_error)?;
        self.file.seek(SeekFrom::Start(0)).map_err(storage_error)?;
        Ok(())
    }
    /// Streaming serde sequence retains one accepted cell (at most 16 MiB),
    /// never the entire accepted 256 MiB original JSON row or all typed cells.
    pub(super) fn row(
        &mut self,
        file: &mut std::fs::File,
        table: &HistoricalHistoryTable,
        position: u64,
        rowid: &str,
        original_sha256: &str,
    ) -> Result<()> {
        file.seek(SeekFrom::Start(0)).map_err(storage_error)?;
        let mut context = RowContext {
            stage: self,
            table,
            position,
            rowid,
            columns: 0,
            current: vec![],
            pins: vec![],
        };
        let mut decoder = serde_json::Deserializer::from_reader(std::io::BufReader::new(file));
        RowSeed(&mut context)
            .deserialize(&mut decoder)
            .map_err(|_| Error::Conflict)?;
        decoder.end().map_err(|_| Error::Conflict)?;
        if context.columns != table.columns.len() {
            return Err(Error::Conflict);
        }
        context.flush()?;
        let row = HistoricalHistoryRow {
            position,
            rowid: rowid.into(),
            original_sha256: original_sha256.into(),
            columns: u32::try_from(context.columns).map_err(|_| Error::BudgetExceeded)?,
            cell_pages: std::mem::take(&mut context.pins),
        };
        context.stage.put_json(
            &format!("history:row:{}", table.ordinal),
            i64::try_from(position - 1).map_err(|_| Error::Conflict)?,
            &row,
        )?;
        context.stage.rows += 1;
        Ok(())
    }
}
struct RowContext<'a> {
    stage: &'a mut HistoryStage,
    table: &'a HistoricalHistoryTable,
    position: u64,
    rowid: &'a str,
    columns: usize,
    current: Vec<HistoricalHistoryColumn>,
    pins: Vec<HistoricalHistoryPagePin>,
}
impl RowContext<'_> {
    fn flush(&mut self) -> Result<()> {
        if self.current.is_empty() {
            return Ok(());
        }
        let page = HistoricalHistoryCellsPage {
            table_ordinal: self.table.ordinal,
            position: self.position,
            rowid: self.rowid.into(),
            page_index: self.pins.len() as u32,
            cells: std::mem::take(&mut self.current),
        };
        self.pins.push(self.stage.put_json(
            &format!("history:cells:{}:{}", self.table.ordinal, self.position),
            page.page_index as i64,
            &page,
        )?);
        Ok(())
    }
    fn cell(&mut self, value: Value) -> Result<()> {
        if self.columns >= self.table.columns.len() {
            return Err(Error::Conflict);
        }
        let map = value.as_object().ok_or(Error::Conflict)?;
        let kind = value["type"].as_str().ok_or(Error::Conflict)?;
        let expected: &[&str] = match kind {
            "null" => &["type"],
            "integer" => &["type", "decimal"],
            "real" => &["type", "ieee754_binary64_be"],
            "text" | "blob" => &["type", "base64"],
            _ => return Err(Error::Conflict),
        };
        if map.len() != expected.len() || expected.iter().any(|k| !map.contains_key(*k)) {
            return Err(Error::Conflict);
        }
        let cell = match kind {
            "null" => HistoricalHistoryCell::Null,
            "integer" => {
                let decimal = value["decimal"].as_str().ok_or(Error::Conflict)?;
                let n = decimal.parse::<i64>().map_err(|_| Error::Conflict)?;
                if n.to_string() != decimal {
                    return Err(Error::Conflict);
                }
                HistoricalHistoryCell::Integer {
                    decimal: decimal.into(),
                }
            }
            "real" => {
                let bits = value["ieee754_binary64_be"]
                    .as_str()
                    .ok_or(Error::Conflict)?;
                if bits.len() != 16
                    || !bits
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || !u64::from_str_radix(bits, 16).is_ok_and(|n| f64::from_bits(n).is_finite())
                {
                    return Err(Error::Conflict);
                }
                HistoricalHistoryCell::RealBits {
                    ieee754_binary64_be: bits.into(),
                }
            }
            "text" | "blob" => {
                let encoded = value["base64"].as_str().ok_or(Error::Conflict)?;
                if encoded.len() > (16 * 1024 * 1024usize).div_ceil(3) * 4 {
                    return Err(Error::BudgetExceeded);
                }
                let bytes = Zeroizing::new(STANDARD.decode(encoded).map_err(storage_error)?);
                if bytes.len() > 16 * 1024 * 1024 || STANDARD.encode(bytes.as_slice()) != encoded {
                    return Err(Error::Conflict);
                }
                let mut pages = vec![];
                for (page, raw) in bytes.chunks(CHUNK_BYTES).enumerate() {
                    pages.push(self.stage.put(
                        &format!(
                            "history:value:{}:{}:{}",
                            self.table.ordinal, self.position, self.columns
                        ),
                        page as i64,
                        raw,
                    )?);
                }
                self.stage.value_bytes = self
                    .stage
                    .value_bytes
                    .checked_add(bytes.len() as u64)
                    .ok_or(Error::BudgetExceeded)?;
                let size = bytes.len() as u64;
                let sha256 = hash(&bytes);
                let preview_base64 = STANDARD.encode(&bytes[..bytes.len().min(96)]);
                if kind == "text" {
                    HistoricalHistoryCell::TextBytes {
                        size,
                        sha256,
                        pages,
                        preview_base64,
                    }
                } else {
                    HistoricalHistoryCell::Blob {
                        size,
                        sha256,
                        pages,
                        preview_base64,
                    }
                }
            }
            _ => return Err(Error::Conflict),
        };
        self.current.push(HistoricalHistoryColumn {
            column_index: self.columns as u32,
            value: cell,
        });
        self.columns += 1;
        self.stage.cells += 1;
        if self.current.len() == CELLS_PER_PAGE {
            self.flush()?;
        }
        Ok(())
    }
}
struct RowSeed<'a, 'b>(&'a mut RowContext<'b>);
impl<'de> DeserializeSeed<'de> for RowSeed<'_, '_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for RowSeed<'_, '_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("original canonical typed row")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> std::result::Result<(), M::Error> {
        let mut cells = false;
        let mut rowid = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "cells" if !cells => {
                    map.next_value_seed(CellsSeed(&mut *self.0))?;
                    cells = true;
                }
                "rowid" if !rowid => {
                    let value = map.next_value::<String>()?;
                    if value != self.0.rowid {
                        return Err(serde::de::Error::custom("original rowid differs"));
                    }
                    rowid = true;
                }
                _ => return Err(serde::de::Error::custom("original row fields differ")),
            }
        }
        if !cells || !rowid {
            return Err(serde::de::Error::custom("original row fields missing"));
        }
        Ok(())
    }
}
struct CellsSeed<'a, 'b>(&'a mut RowContext<'b>);
impl<'de> DeserializeSeed<'de> for CellsSeed<'_, '_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for CellsSeed<'_, '_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("original typed cell sequence")
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> std::result::Result<(), S::Error> {
        while let Some(value) = seq.next_element::<Value>()? {
            self.0.cell(value).map_err(|_| {
                serde::de::Error::custom("original cell or current storage budget refused")
            })?;
        }
        Ok(())
    }
}
