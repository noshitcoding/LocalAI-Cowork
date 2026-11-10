//! Independent domain for immutable original intake and frozen activation plans.
use super::types::{CHUNK_BYTES, MAX_PROJECTED_BYTES};
use crate::{Error, Result, hash, json_string, parse, storage_error};
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_secrets::{KeyId, KeyStore};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Row, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Sealed {
    pub format: u32,
    pub key_id: KeyId,
    pub nonce: String,
    pub ciphertext: String,
}
pub(crate) fn seal(
    keys: &dyn KeyStore,
    key_id: KeyId,
    owner: &str,
    import: Uuid,
    stream: &str,
    index: i64,
    sha: &str,
    size: u64,
    bytes: &[u8],
) -> Result<String> {
    if size != bytes.len() as u64 || hash(bytes) != sha || bytes.len() > MAX_PROJECTED_BYTES {
        return Err(Error::Conflict);
    }
    let master = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, master.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let aad = json_string(&(
        "historical-import-cipher/v1",
        owner,
        import,
        stream,
        index,
        sha,
        size,
        key_id,
    ))?;
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let mut raw = Zeroizing::new(bytes.to_vec());
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(aad.as_bytes()),
        &mut *raw,
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    json_string(&Sealed {
        format: 1,
        key_id,
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(raw.as_slice()),
    })
}
pub(crate) fn open(
    keys: &dyn KeyStore,
    owner: &str,
    import: Uuid,
    stream: &str,
    index: i64,
    sha: &str,
    size: u64,
    sealed: &str,
) -> Result<Zeroizing<Vec<u8>>> {
    if size > MAX_PROJECTED_BYTES as u64 || sealed.len() > MAX_PROJECTED_BYTES * 2 {
        return Err(Error::BudgetExceeded);
    }
    let v: Sealed = parse(sealed)?;
    if v.format != 1 {
        return Err(Error::Conflict);
    }
    let nonce: [u8; 12] = STANDARD
        .decode(&v.nonce)
        .map_err(storage_error)?
        .try_into()
        .map_err(|_| Error::Conflict)?;
    let mut raw = Zeroizing::new(STANDARD.decode(&v.ciphertext).map_err(storage_error)?);
    if raw.len() != size as usize + 16 {
        return Err(Error::Conflict);
    }
    let master = keys
        .load(&v.key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, master.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let aad = json_string(&(
        "historical-import-cipher/v1",
        owner,
        import,
        stream,
        index,
        sha,
        size,
        v.key_id,
    ))?;
    let plain = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad.as_bytes()),
            &mut raw,
        )
        .map_err(|_| Error::Conflict)?;
    if plain.len() != size as usize || hash(plain) != sha {
        return Err(Error::Conflict);
    }
    Ok(Zeroizing::new(plain.to_vec()))
}
pub(crate) async fn put(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key_id: KeyId,
    owner: &str,
    import: Uuid,
    stream: &str,
    index: i64,
    bytes: &[u8],
) -> Result<()> {
    let digest = hash(bytes);
    let raw = seal(
        keys,
        key_id,
        owner,
        import,
        stream,
        index,
        &digest,
        bytes.len() as u64,
        bytes,
    )?;
    sqlx::query("INSERT INTO historical_import_chunks(import_id,owner,stream,chunk_index,size,sha256,sealed) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(import.to_string()).bind(owner).bind(stream).bind(index).bind(bytes.len() as i64).bind(digest).bind(&raw).execute(&mut **tx).await.map_err(storage_error)?;
    super::key_references::put(tx, owner, import, stream, index, key_id, &raw).await?;
    Ok(())
}
pub(crate) async fn get(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: &str,
    import: Uuid,
    stream: &str,
    index: i64,
) -> Result<Zeroizing<Vec<u8>>> {
    let r=sqlx::query("SELECT owner,size,sha256,sealed FROM historical_import_chunks WHERE import_id=$1 AND stream=$2 AND chunk_index=$3")
        .bind(import.to_string()).bind(stream).bind(index).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    if r.try_get::<String, _>("owner").map_err(storage_error)? != owner {
        return Err(Error::Forbidden);
    }
    let size: i64 = r.try_get("size").map_err(storage_error)?;
    let size = u64::try_from(size).map_err(|_| Error::Conflict)?;
    let original: String = r.try_get("sealed").map_err(storage_error)?;
    super::key_references::check(tx, owner, import, stream, index, &original).await?;
    open(
        keys,
        owner,
        import,
        stream,
        index,
        &r.try_get::<String, _>("sha256").map_err(storage_error)?,
        size,
        &original,
    )
}
pub(crate) async fn put_pages(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key_id: KeyId,
    owner: &str,
    import: Uuid,
    stream: &str,
    bytes: &[u8],
) -> Result<Vec<super::types::HistoricalContentPage>> {
    let mut pages = vec![];
    for (index, page) in bytes.chunks(CHUNK_BYTES).enumerate() {
        put(tx, keys, key_id, owner, import, stream, index as i64, page).await?;
        pages.push(super::types::HistoricalContentPage {
            stream: stream.to_owned(),
            chunk_index: index as u32,
            size: page.len() as u64,
            sha256: hash(page),
        });
    }
    Ok(pages)
}
pub(crate) async fn get_pages(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: &str,
    import: Uuid,
    stream: &str,
) -> Result<Zeroizing<Vec<u8>>> {
    let rows=sqlx::query("SELECT chunk_index,size FROM historical_import_chunks WHERE import_id=$1 AND owner=$2 AND stream=$3 ORDER BY chunk_index LIMIT 257").bind(import.to_string()).bind(owner).bind(stream).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.is_empty() || rows.len() > 256 {
        return Err(Error::BudgetExceeded);
    }
    let mut result = Zeroizing::new(Vec::new());
    for (index, r) in rows.iter().enumerate() {
        if r.try_get::<i64, _>("chunk_index").map_err(storage_error)? != index as i64 {
            return Err(Error::Conflict);
        }
        let bytes = get(tx, keys, owner, import, stream, index as i64).await?;
        if bytes.len() > CHUNK_BYTES || (index + 1 < rows.len() && bytes.len() != CHUNK_BYTES) {
            return Err(Error::Conflict);
        }
        result.extend_from_slice(&bytes);
    }
    Ok(result)
}
/// Original plaintext/digest/AAD remain exact; only cipher custody changes.
pub(crate) async fn rewrap(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
    native: bool,
) -> Result<u64> {
    let owner = if native {
        format!("native:{}", target.instance)
    } else {
        String::new()
    };
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM historical_import_chunks WHERE ($1=1 AND owner=$2) OR ($1=0 AND owner LIKE 'org:%')")
        .bind(i64::from(native)).bind(&owner)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    if total < 0 {
        return Err(Error::BudgetExceeded);
    }
    let mut after_import = String::new();
    let mut after_stream = String::new();
    let mut after_index = -1i64;
    let mut observed = 0i64;
    let mut count = 0;
    loop {
        let rows=sqlx::query("SELECT import_id,owner,stream,chunk_index,size,sha256,sealed FROM historical_import_chunks WHERE (($4=1 AND owner=$5) OR ($4=0 AND owner LIKE 'org:%')) AND (import_id>$1 OR (import_id=$1 AND stream>$2) OR (import_id=$1 AND stream=$2 AND chunk_index>$3)) ORDER BY import_id,stream,chunk_index LIMIT 16")
            .bind(&after_import).bind(&after_stream).bind(after_index).bind(i64::from(native)).bind(&owner).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for r in rows {
            let import_text: String = r.try_get("import_id").map_err(storage_error)?;
            let import = Uuid::parse_str(&import_text).map_err(storage_error)?;
            let owner: String = r.try_get("owner").map_err(storage_error)?;
            let stream: String = r.try_get("stream").map_err(storage_error)?;
            let index: i64 = r.try_get("chunk_index").map_err(storage_error)?;
            let size: i64 = r.try_get("size").map_err(storage_error)?;
            let sha: String = r.try_get("sha256").map_err(storage_error)?;
            let original: String = r.try_get("sealed").map_err(storage_error)?;
            observed += 1;
            if observed > total {
                return Err(Error::Conflict);
            }
            after_import = import_text.clone();
            after_stream = stream.clone();
            after_index = index;
            let header: Sealed = parse(&original)?;
            super::key_references::check(tx, &owner, import, &stream, index, &original).await?;
            if header.key_id.instance != target.instance {
                return Err(Error::NativeKeyUnavailable);
            }
            if header.key_id == target {
                continue;
            }
            let size = u64::try_from(size).map_err(|_| Error::Conflict)?;
            if size > CHUNK_BYTES as u64 {
                return Err(Error::Conflict);
            }
            let plain = open(keys, &owner, import, &stream, index, &sha, size, &original)?;
            let next = seal(
                keys, target, &owner, import, &stream, index, &sha, size, &plain,
            )?;
            let changed=sqlx::query("UPDATE historical_import_chunks SET sealed=$1 WHERE import_id=$2 AND owner=$3 AND stream=$4 AND chunk_index=$5 AND size=$6 AND sha256=$7 AND sealed=$8")
            .bind(&next).bind(import_text).bind(&owner).bind(&stream).bind(index).bind(size as i64).bind(sha).bind(&original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
            if changed != 1 {
                return Err(Error::Conflict);
            }
            super::key_references::replace(
                tx,
                &owner,
                import,
                &stream,
                index,
                header.key_id,
                target,
                &original,
                &next,
            )
            .await?;
            count += 1;
        }
    }
    if observed != total {
        return Err(Error::Conflict);
    }
    Ok(count)
}
