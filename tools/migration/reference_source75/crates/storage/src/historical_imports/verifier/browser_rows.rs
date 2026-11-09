//! Bounded original-row capture for the inert browser index. Never live authority.
use super::*;

#[derive(Clone, Debug)]
pub struct VerifiedHistoricalTable {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: u64,
}
#[derive(Clone, Debug)]
pub struct HistoricalBrowserRow {
    pub source_table: String,
    pub source_rowid: i64,
    pub position: u64,
    pub canonical_bytes: u64,
    pub canonical_sha256: String,
}
pub struct HistoricalBrowserRows {
    connection: HistoricalConnection,
    tables: Vec<TableProof>,
    table: usize,
    position: usize,
    current: Option<HistoricalBrowserRow>,
    current_verified: bool,
    budget: Budget,
}
impl HistoricalBrowserRows {
    pub(super) async fn open(archive: &VerifiedHistoryArchive) -> Result<Self> {
        let budget = Budget::new();
        let connection = connection(&archive.stage, "quarantine.sqlite3", true, budget).await?;
        Ok(Self {
            connection,
            tables: archive
                .tables
                .iter()
                .map(|table| TableProof {
                    name: table.name.clone(),
                    columns: table.columns.clone(),
                    rows: table.rows.clone(),
                })
                .collect(),
            table: 0,
            position: 0,
            current: None,
            current_verified: true,
            budget,
        })
    }
    pub async fn next(&mut self) -> Result<Option<HistoricalBrowserRow>> {
        if !self.current_verified {
            return Err(Error::Refused(
                "Original row must be completely captured and verified before advancing",
            ));
        }
        // A fresh bound covers exactly the next original row. It cannot renew
        // a partially captured row and does not re-open/reverify the archive.
        // The held proof bounds the total number of rows independently.
        self.budget = Budget::new();
        let deadline = self.budget.deadline;
        self.connection
            .lock_handle()
            .await?
            .set_progress_handler(1000, move || Instant::now() <= deadline);
        self.budget.check()?;
        self.current = None;
        while self.table < self.tables.len() {
            let table = &self.tables[self.table];
            if self.position == table.rows.len() {
                self.table += 1;
                self.position = 0;
                continue;
            }
            let position = self.position + 1;
            let sha = c::hex(&table.rows[self.position]);
            let row = sqlx::query("SELECT source_rowid,length(CAST(canonical_row AS BLOB)),row_sha256 FROM history_rows WHERE table_name=? AND position=?")
                .bind(&table.name).bind(position as i64).fetch_optional(&mut *self.connection).await?
                .ok_or(Error::Refused("Verified original row header is missing"))?;
            let size = row.try_get::<i64, _>(1)?;
            if size <= 0 || size > MAX_LINE as i64 || row.try_get::<String, _>(2)? != sha {
                return Err(Error::Refused(
                    "Original row header differs from held verification proof",
                ));
            }
            let header = HistoricalBrowserRow {
                source_table: table.name.clone(),
                source_rowid: row.try_get(0)?,
                position: position as u64,
                canonical_bytes: size as u64,
                canonical_sha256: sha,
            };
            self.position = position;
            self.current = Some(header.clone());
            self.current_verified = false;
            return Ok(Some(header));
        }
        Ok(None)
    }
    /// Output is only a tentative private capture until this method returns success.
    /// Callers must not publish its bytes or an index receipt before whole-row verification.
    pub async fn write_current_to(&mut self, output: &mut (dyn Write + Send)) -> Result<()> {
        self.budget.check()?;
        if self.current_verified {
            return Err(Error::Refused("No undelivered original row is selected"));
        }
        let current = self
            .current
            .as_ref()
            .ok_or(Error::Refused("Original row is not selected"))?
            .clone();
        let mut offset = 0u64;
        let mut digest = Sha256::new();
        while offset < current.canonical_bytes {
            self.budget.check()?;
            let count = (current.canonical_bytes - offset).min(1024 * 1024);
            let bytes: Vec<u8> = sqlx::query_scalar("SELECT substr(CAST(canonical_row AS BLOB),?,?) FROM history_rows WHERE table_name=? AND position=? AND source_rowid=? AND row_sha256=? AND length(CAST(canonical_row AS BLOB))=?")
                .bind((offset + 1) as i64).bind(count as i64).bind(&current.source_table)
                .bind(current.position as i64).bind(current.source_rowid).bind(&current.canonical_sha256)
                .bind(current.canonical_bytes as i64).fetch_optional(&mut *self.connection).await?
                .ok_or(Error::Refused("Original row changed during bounded capture"))?;
            if bytes.len() != count as usize {
                return Err(Error::Refused("Original row segment is incomplete"));
            }
            digest.update(&bytes);
            output.write_all(&bytes)?;
            offset += count;
        }
        if c::hex(&digest.finalize()) != current.canonical_sha256 {
            return Err(Error::Refused(
                "Captured original row SHA differs from held verification proof",
            ));
        }
        output.flush()?;
        self.current_verified = true;
        Ok(())
    }
    pub async fn close(self) -> Result<()> {
        self.connection.close().await?;
        Ok(())
    }
}
