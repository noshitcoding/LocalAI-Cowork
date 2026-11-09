//! Bounded cache encryption; rewrap keeps all original hashes and business revisions.
use super::*;
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
fn aad(
    owner: Uuid,
    id: Uuid,
    stream: &str,
    ordinal: i64,
    digest: &str,
    size: usize,
    key: KeyId,
) -> Result<String> {
    json_string(&(
        "android-mobile-cache/v1",
        owner,
        id,
        stream,
        ordinal,
        digest,
        size,
        key,
    ))
}
pub(crate) fn seal(
    keys: &dyn KeyStore,
    key: KeyId,
    owner: Uuid,
    id: Uuid,
    stream: &str,
    ordinal: i64,
    raw: &[u8],
) -> Result<Value> {
    if raw.len() > CACHE_PART || ordinal < 0 || stream.len() > 200 {
        return Err(device_invalid());
    }
    let loaded = keys.load(&key).map_err(|_| Error::NativeKeyUnavailable)?;
    let crypto = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, loaded.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let digest = hash(raw);
    let aad = aad(owner, id, stream, ordinal, &digest, raw.len(), key)?;
    let mut cipher = Zeroizing::new(raw.to_vec());
    crypto
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad.as_bytes()),
            &mut *cipher,
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
    Ok(
        json!({"format":1,"key_id":key,"nonce":STANDARD.encode(nonce),"ciphertext":STANDARD.encode(cipher.as_slice())}),
    )
}
pub(crate) fn open(keys: &dyn KeyStore, owner: Uuid, row: &AnyRow) -> Result<Zeroizing<Vec<u8>>> {
    let stored: Value = parse(&row.try_get::<String, _>("sealed").map_err(storage_error)?)?;
    if stored["format"] != 1 {
        return Err(Error::Forbidden);
    }
    let id = Uuid::parse_str(
        &row.try_get::<String, _>("import_id")
            .map_err(storage_error)?,
    )
    .map_err(storage_error)?;
    let stream: String = row.try_get("stream").map_err(storage_error)?;
    let ordinal: i64 = row.try_get("ordinal").map_err(storage_error)?;
    let size: i64 = row.try_get("size").map_err(storage_error)?;
    if !(0..=CACHE_PART as i64).contains(&size) {
        return Err(Error::BudgetExceeded);
    }
    let digest: String = row.try_get("digest").map_err(storage_error)?;
    let key: KeyId = serde_json::from_value(stored["key_id"].clone()).map_err(storage_error)?;
    key.validate().map_err(|_| Error::Forbidden)?;
    let loaded = keys.load(&key).map_err(|_| Error::NativeKeyUnavailable)?;
    let crypto = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, loaded.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let nonce: [u8; 12] = STANDARD
        .decode(stored["nonce"].as_str().ok_or_else(device_invalid)?)
        .map_err(storage_error)?
        .try_into()
        .map_err(|_| device_invalid())?;
    let mut raw = Zeroizing::new(
        STANDARD
            .decode(
                stored["ciphertext"]
                    .as_str()
                    .filter(|s| s.len() <= (CACHE_PART + 16).div_ceil(3) * 4)
                    .ok_or_else(device_invalid)?,
            )
            .map_err(storage_error)?,
    );
    if raw.len() != size as usize + 16 {
        return Err(Error::Forbidden);
    }
    let authenticated = aad(owner, id, &stream, ordinal, &digest, size as usize, key)?;
    let plain = crypto
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(authenticated.as_bytes()),
            &mut raw,
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let len = plain.len();
    if len != size as usize || hash(plain) != digest {
        return Err(Error::Forbidden);
    }
    raw.truncate(len);
    Ok(raw)
}
pub(crate) async fn preflight(tx: &mut Transaction<'_, Any>) -> Result<()> {
    let bad:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_mobile_cache_chunks WHERE size<0 OR size>1048576 OR LENGTH(sealed)>1500000 OR LENGTH(stream)>200 OR ordinal<0").fetch_one(&mut **tx).await.map_err(storage_error)?;
    if bad != 0 {
        Err(Error::BudgetExceeded)
    } else {
        Ok(())
    }
}
pub(crate) async fn required_keys(tx: &mut Transaction<'_, Any>) -> Result<Vec<KeyId>> {
    let mut found = Vec::new();
    let mut last = (String::new(), String::new(), -1i64);
    loop {
        let rows=sqlx::query("SELECT import_id,stream,ordinal,sealed FROM android_mobile_cache_chunks WHERE import_id>$1 OR (import_id=$1 AND stream>$2) OR (import_id=$1 AND stream=$2 AND ordinal>$3) ORDER BY import_id,stream,ordinal LIMIT 100").bind(&last.0).bind(&last.1).bind(last.2).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for r in rows {
            last = (
                r.try_get("import_id").map_err(storage_error)?,
                r.try_get("stream").map_err(storage_error)?,
                r.try_get("ordinal").map_err(storage_error)?,
            );
            let v: Value = parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
            let k: KeyId = serde_json::from_value(v["key_id"].clone()).map_err(storage_error)?;
            k.validate().map_err(|_| Error::Forbidden)?;
            if !found.contains(&k) {
                found.push(k);
            }
        }
    }
    Ok(found)
}
pub(crate) async fn rewrap(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    let mut last = (String::new(), String::new(), -1i64);
    let mut count = 0;
    loop {
        let rows=sqlx::query("SELECT c.*,i.owner_id FROM android_mobile_cache_chunks c JOIN android_mobile_cache_imports i ON i.id=c.import_id WHERE c.import_id>$1 OR (c.import_id=$1 AND c.stream>$2) OR (c.import_id=$1 AND c.stream=$2 AND c.ordinal>$3) ORDER BY c.import_id,c.stream,c.ordinal LIMIT 100").bind(&last.0).bind(&last.1).bind(last.2).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for r in rows {
            last = (
                r.try_get("import_id").map_err(storage_error)?,
                r.try_get("stream").map_err(storage_error)?,
                r.try_get("ordinal").map_err(storage_error)?,
            );
            let old: String = r.try_get("sealed").map_err(storage_error)?;
            let owner =
                Uuid::parse_str(&r.try_get::<String, _>("owner_id").map_err(storage_error)?)
                    .map_err(storage_error)?;
            let parsed: Value = parse(&old)?;
            let original: KeyId =
                serde_json::from_value(parsed["key_id"].clone()).map_err(storage_error)?;
            original.validate().map_err(|_| Error::Forbidden)?;
            if original.instance != target.instance || original == target {
                continue;
            }
            let plain = open(keys, owner, &r)?;
            let id = Uuid::parse_str(&last.0).map_err(storage_error)?;
            let new = seal(keys, target, owner, id, &last.1, last.2, &plain)?;
            if sqlx::query("UPDATE android_mobile_cache_chunks SET sealed=$1 WHERE import_id=$2 AND stream=$3 AND ordinal=$4 AND sealed=$5 AND digest=$6 AND size=$7").bind(json_string(&new)?).bind(&last.0).bind(&last.1).bind(last.2).bind(old).bind(r.try_get::<String,_>("digest").map_err(storage_error)?).bind(r.try_get::<i64,_>("size").map_err(storage_error)?).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
            count += 1;
        }
    }
    Ok(count)
}
