//! Bounded authenticated original content pages; file bytes never enter a JSON catalogue.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_secrets::{KeyId, KeyStore};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use sqlx::{Any, Transaction};
use zeroize::Zeroizing;
pub(super) const PAGE: usize = 1024 * 1024;
pub(super) const MAX_CONTENT: u64 = 256 * 1024 * 1024;
pub(super) const MAX_OBJECT: usize = 16 * 1024 * 1024;

/// Structural preflight is page-bounded; complete authentication occurs during rewrap.
pub(super) fn envelope_key(text: &str, size: i64, digest: &str) -> Result<KeyId> {
    if !(1..=PAGE as i64).contains(&size)
        || text.len() > 2 * PAGE
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Forbidden);
    }
    let value: Value = parse(text)?;
    if value.as_object().is_none_or(|o| o.len() != 4)
        || value["format"] != "android-project-version-page/1"
    {
        return Err(Error::Forbidden);
    }
    let key: KeyId =
        serde_json::from_value(value["key_id"].clone()).map_err(|_| Error::Forbidden)?;
    key.validate().map_err(|_| Error::Forbidden)?;
    let nonce = value["nonce"]
        .as_str()
        .filter(|s| s.len() == 16)
        .ok_or(Error::Forbidden)?;
    if STANDARD.decode(nonce).map_err(|_| Error::Forbidden)?.len() != 12 {
        return Err(Error::Forbidden);
    }
    let encoded = value["ciphertext"]
        .as_str()
        .filter(|s| s.len() <= 2 * PAGE)
        .ok_or(Error::Forbidden)?;
    if STANDARD
        .decode(encoded)
        .map_err(|_| Error::Forbidden)?
        .len()
        != size as usize + 16
    {
        return Err(Error::Forbidden);
    }
    Ok(key)
}

pub(crate) async fn required_keys(tx: &mut Transaction<'_, Any>) -> Result<Vec<KeyId>> {
    let mut found = BTreeMap::new();
    let mut after = (String::new(), String::new(), -1i64);
    loop {
        // CASE prevents an oversized corrupt envelope from being loaded into the client heap.
        let row = sqlx::query("SELECT c.version_id,c.piece_key,c.ordinal,c.digest,c.size,LENGTH(c.sealed) AS sealed_length,CASE WHEN LENGTH(c.sealed)<=2097152 THEN c.sealed ELSE NULL END AS sealed,v.creator_id,v.project_id FROM android_project_version_parts c LEFT JOIN android_project_versions v ON v.id=c.version_id WHERE c.version_id>$1 OR (c.version_id=$1 AND c.piece_key>$2) OR (c.version_id=$1 AND c.piece_key=$2 AND c.ordinal>$3) ORDER BY c.version_id,c.piece_key,c.ordinal LIMIT 1").bind(&after.0).bind(&after.1).bind(after.2).fetch_optional(&mut **tx).await.map_err(storage_error)?;
        let Some(row) = row else { break };
        let version: String = row.try_get("version_id").map_err(storage_error)?;
        let piece: String = row.try_get("piece_key").map_err(storage_error)?;
        let ordinal: i64 = row.try_get("ordinal").map_err(storage_error)?;
        Uuid::parse_str(&version).map_err(|_| Error::Forbidden)?;
        row_id(&row, "creator_id")?;
        row_id(&row, "project_id")?;
        if piece.is_empty()
            || piece.len() > 300
            || !(0..256).contains(&ordinal)
            || row
                .try_get::<i64, _>("sealed_length")
                .map_err(storage_error)?
                > (2 * PAGE) as i64
        {
            return Err(Error::Forbidden);
        }
        let key = envelope_key(
            &row.try_get::<Option<String>, _>("sealed")
                .map_err(storage_error)?
                .ok_or(Error::Forbidden)?,
            row.try_get("size").map_err(storage_error)?,
            &row.try_get::<String, _>("digest").map_err(storage_error)?,
        )?;
        found.insert((key.instance, key.key), key);
        after = (version, piece, ordinal);
    }
    for key in super::snapshots::required_keys(tx).await? {
        found.insert((key.instance, key.key), key);
    }
    Ok(found.into_values().collect())
}
pub(crate) async fn ciphertext_preflight(tx: &mut Transaction<'_, Any>) -> Result<()> {
    required_keys(tx).await.map(|_| ())
}

pub(super) struct Scope {
    pub owner: Uuid,
    pub project: Uuid,
    pub version: Uuid,
}
fn aad(
    scope: &Scope,
    piece: &str,
    ordinal: i64,
    digest: &str,
    size: usize,
    key: KeyId,
) -> Result<Vec<u8>> {
    serde_json::to_vec(&(
        "android-project-version-content/v1",
        scope.owner,
        scope.project,
        scope.version,
        piece,
        ordinal,
        digest,
        size,
        key,
    ))
    .map_err(storage_error)
}
pub(super) fn seal(
    keys: &dyn KeyStore,
    key_id: KeyId,
    scope: &Scope,
    piece: &str,
    ordinal: i64,
    bytes: &[u8],
) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > PAGE {
        return Err(Error::BudgetExceeded);
    }
    let digest = hash(bytes);
    let master = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, master.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let mut encrypted = Zeroizing::new(bytes.to_vec());
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(aad(scope, piece, ordinal, &digest, bytes.len(), key_id)?),
        &mut *encrypted,
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    Ok(
        json!({"format":"android-project-version-page/1","key_id":key_id,"nonce":STANDARD.encode(nonce),"ciphertext":STANDARD.encode(encrypted.as_slice())}),
    )
}
pub(super) fn open(
    keys: &dyn KeyStore,
    scope: &Scope,
    piece: &str,
    ordinal: i64,
    digest: &str,
    size: usize,
    sealed: &Value,
) -> Result<Zeroizing<Vec<u8>>> {
    if sealed["format"] != "android-project-version-page/1"
        || size == 0
        || size > PAGE
        || ordinal < 0
    {
        return Err(Error::Forbidden);
    }
    let key_id: KeyId =
        serde_json::from_value(sealed["key_id"].clone()).map_err(|_| Error::Forbidden)?;
    key_id.validate().map_err(|_| Error::Forbidden)?;
    let master = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let nonce: [u8; 12] = STANDARD
        .decode(sealed["nonce"].as_str().ok_or(Error::Forbidden)?)
        .map_err(|_| Error::Forbidden)?
        .try_into()
        .map_err(|_| Error::Forbidden)?;
    let mut bytes = Zeroizing::new(
        STANDARD
            .decode(sealed["ciphertext"].as_str().ok_or(Error::Forbidden)?)
            .map_err(|_| Error::Forbidden)?,
    );
    if bytes.len() != size + 16 {
        return Err(Error::Forbidden);
    }
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, master.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let plain = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad(scope, piece, ordinal, digest, size, key_id)?),
            &mut bytes,
        )
        .map_err(|_| Error::Forbidden)?
        .to_vec();
    if plain.len() != size || hash(&plain) != digest {
        return Err(Error::Forbidden);
    }
    Ok(Zeroizing::new(plain))
}
pub(super) async fn write_page(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    scope: &Scope,
    piece: &str,
    ordinal: i64,
    bytes: &[u8],
) -> Result<()> {
    if piece.is_empty() || piece.len() > 300 || ordinal < 0 {
        return Err(Error::Forbidden);
    }
    let sealed = seal(keys, key, scope, piece, ordinal, bytes)?;
    crate::server_keys::cipher_writer(tx, key).await?;
    sqlx::query("INSERT INTO android_project_version_parts(version_id,piece_key,ordinal,digest,size,sealed) VALUES($1,$2,$3,$4,$5,$6)").bind(scope.version.to_string()).bind(piece).bind(ordinal).bind(hash(bytes)).bind(bytes.len() as i64).bind(json_string(&sealed)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(super) async fn read_page(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    scope: &Scope,
    piece: &str,
    ordinal: i64,
) -> Result<Zeroizing<Vec<u8>>> {
    let r=sqlx::query("SELECT digest,size,sealed FROM android_project_version_parts WHERE version_id=$1 AND piece_key=$2 AND ordinal=$3").bind(scope.version.to_string()).bind(piece).bind(ordinal).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    open(
        keys,
        scope,
        piece,
        ordinal,
        &r.try_get::<String, _>("digest").map_err(storage_error)?,
        usize::try_from(r.try_get::<i64, _>("size").map_err(storage_error)?)
            .map_err(|_| Error::Forbidden)?,
        &parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?,
    )
}
pub(super) fn piece(kind: &str, id: Uuid) -> String {
    format!("object:{kind}:{id}")
}
pub(super) async fn write_object(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    scope: &Scope,
    kind: &str,
    id: Uuid,
    value: &Value,
) -> Result<()> {
    let bytes = Zeroizing::new(serde_json::to_vec(value).map_err(storage_error)?);
    if bytes.len() > MAX_OBJECT {
        return Err(Error::BudgetExceeded);
    }
    let name = piece(kind, id);
    for (ordinal, page) in bytes.chunks(PAGE).enumerate() {
        write_page(tx, keys, key, scope, &name, ordinal as i64, page).await?;
    }
    sqlx::query("INSERT INTO android_project_version_objects(version_id,kind,source_id,payload_sha256,size,part_count) VALUES($1,$2,$3,$4,$5,$6)").bind(scope.version.to_string()).bind(kind).bind(id.to_string()).bind(hash(&bytes)).bind(bytes.len() as i64).bind(bytes.len().div_ceil(PAGE) as i64).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(super) async fn read_object(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    scope: &Scope,
    kind: &str,
    id: Uuid,
) -> Result<Value> {
    let row=sqlx::query("SELECT payload_sha256,size,part_count FROM android_project_version_objects WHERE version_id=$1 AND kind=$2 AND source_id=$3").bind(scope.version.to_string()).bind(kind).bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let size = usize::try_from(row.try_get::<i64, _>("size").map_err(storage_error)?)
        .map_err(|_| Error::Forbidden)?;
    let count = row.try_get::<i64, _>("part_count").map_err(storage_error)?;
    if size > MAX_OBJECT || count != size.div_ceil(PAGE) as i64 {
        return Err(Error::Forbidden);
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(size));
    let name = piece(kind, id);
    for ordinal in 0..count {
        let page = read_page(tx, keys, scope, &name, ordinal).await?;
        if bytes.len() + page.len() > size {
            return Err(Error::Forbidden);
        }
        bytes.extend_from_slice(&page);
    }
    if bytes.len() != size
        || hash(&bytes)
            != row
                .try_get::<String, _>("payload_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    serde_json::from_slice(&bytes).map_err(storage_error)
}

/// Privileged master rotation; original business bytes and all version hashes stay unchanged.
pub(crate) async fn rewrap_helper(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    let mut after: Option<(String, String, i64)> = None;
    let mut count = 0u64;
    loop {
        let (v, p, o) = after
            .clone()
            .unwrap_or_else(|| (String::new(), String::new(), -1));
        // One ciphertext page at a time, never a fetch_all of the archive's binary contents.
        let row=sqlx::query("SELECT c.version_id,c.piece_key,c.ordinal,c.digest,c.size,c.sealed,v.creator_id,v.project_id FROM android_project_version_parts c JOIN android_project_versions v ON v.id=c.version_id WHERE c.version_id>$1 OR (c.version_id=$1 AND c.piece_key>$2) OR (c.version_id=$1 AND c.piece_key=$2 AND c.ordinal>$3) ORDER BY c.version_id,c.piece_key,c.ordinal LIMIT 1").bind(v).bind(p).bind(o).fetch_optional(&mut **tx).await.map_err(storage_error)?;
        let Some(row) = row else { break };
        let version: String = row.try_get("version_id").map_err(storage_error)?;
        let name: String = row.try_get("piece_key").map_err(storage_error)?;
        let ordinal: i64 = row.try_get("ordinal").map_err(storage_error)?;
        after = Some((version.clone(), name.clone(), ordinal));
        let original: String = row.try_get("sealed").map_err(storage_error)?;
        let value: Value = parse(&original)?;
        let key: KeyId = serde_json::from_value(value["key_id"].clone())
            .map_err(|_| Error::NativeKeyUnavailable)?;
        if key.instance != target.instance {
            return Err(Error::NativeKeyUnavailable);
        }
        let scope = Scope {
            owner: Uuid::parse_str(
                &row.try_get::<String, _>("creator_id")
                    .map_err(storage_error)?,
            )
            .map_err(storage_error)?,
            project: Uuid::parse_str(
                &row.try_get::<String, _>("project_id")
                    .map_err(storage_error)?,
            )
            .map_err(storage_error)?,
            version: Uuid::parse_str(&version).map_err(storage_error)?,
        };
        let digest: String = row.try_get("digest").map_err(storage_error)?;
        let size: i64 = row.try_get("size").map_err(storage_error)?;
        let plain = open(
            keys,
            &scope,
            &name,
            ordinal,
            &digest,
            usize::try_from(size).map_err(|_| Error::Forbidden)?,
            &value,
        )?;
        // A no-op rotation still authenticates the original AAD, MAC, length and digest.
        if key == target {
            continue;
        }
        let sealed = seal(keys, target, &scope, &name, ordinal, &plain)?;
        let changed=sqlx::query("UPDATE android_project_version_parts SET sealed=$1 WHERE version_id=$2 AND piece_key=$3 AND ordinal=$4 AND digest=$5 AND size=$6 AND sealed=$7").bind(json_string(&sealed)?).bind(version).bind(name).bind(ordinal).bind(digest).bind(size).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        count = count.checked_add(1).ok_or(Error::BudgetExceeded)?;
    }
    count
        .checked_add(super::snapshots::rewrap_helper(tx, keys, target).await?)
        .ok_or(Error::BudgetExceeded)
}
