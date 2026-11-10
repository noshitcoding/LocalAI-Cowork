use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
const PAGE: usize = 1024 * 1024;
fn aad(r: &AnyRow, key: KeyId) -> Result<String> {
    json_string(
        &json!({"format":"android-private-workflow-registry-chunk/1","id":uuid(r,"id")?,"owner":uuid(r,"owner_id")?,"project":uuid(r,"project_id")?,"object":uuid(r,"object_id")?,"purpose":r.try_get::<String,_>("purpose").map_err(storage_error)?,"ordinal":r.try_get::<i64,_>("ordinal").map_err(storage_error)?,"size":r.try_get::<i64,_>("size").map_err(storage_error)?,"digest":r.try_get::<String,_>("digest").map_err(storage_error)?,"key_id":key}),
    )
}
fn seal(keys: &dyn KeyStore, key: KeyId, aad: &str, bytes: &[u8]) -> Result<Value> {
    let keybytes = keys.load(&key).map_err(|_| Error::NativeKeyUnavailable)?;
    let keyring = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, keybytes.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let mut raw = Zeroizing::new(bytes.to_vec());
    keyring
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad.as_bytes()),
            &mut *raw,
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
    Ok(
        json!({"format":1,"key_id":key,"nonce":STANDARD.encode(nonce),"ciphertext":STANDARD.encode(raw.as_slice())}),
    )
}
fn open(keys: &dyn KeyStore, r: &AnyRow, v: &Value) -> Result<Zeroizing<Vec<u8>>> {
    if v["format"] != 1 {
        return Err(Error::Forbidden);
    }
    let key: KeyId = serde_json::from_value(v["key_id"].clone()).map_err(storage_error)?;
    let kb = keys.load(&key).map_err(|_| Error::NativeKeyUnavailable)?;
    let k = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, kb.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let size = r.try_get::<i64, _>("size").map_err(storage_error)?;
    if !(1..=PAGE as i64).contains(&size) {
        return Err(Error::Forbidden);
    }
    let encoded = v["ciphertext"]
        .as_str()
        .filter(|s| s.len() <= 2 * PAGE)
        .ok_or(Error::Forbidden)?;
    let mut raw = Zeroizing::new(STANDARD.decode(encoded).map_err(|_| Error::Forbidden)?);
    if raw.len() != size as usize + 16 {
        return Err(Error::Forbidden);
    }
    let nonce: [u8; 12] = STANDARD
        .decode(v["nonce"].as_str().ok_or(Error::Forbidden)?)
        .map_err(|_| Error::Forbidden)?
        .try_into()
        .map_err(|_| Error::Forbidden)?;
    let a = aad(r, key)?;
    let plain = k
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(a.as_bytes()),
            &mut raw,
        )
        .map_err(|_| Error::Forbidden)?;
    if hash(plain) != r.try_get::<String, _>("digest").map_err(storage_error)? {
        return Err(Error::Forbidden);
    }
    Ok(Zeroizing::new(plain.to_vec()))
}
pub(super) async fn write(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    object: Uuid,
    purpose: &str,
    bytes: &[u8],
    keys: &dyn KeyStore,
    key: KeyId,
) -> Result<()> {
    let key = keys
        .key_for_purpose(&key, cowork_secrets::KeyPurpose::DataEncryption)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    crate::server_keys::cipher_writer(tx, key).await?;
    for (ordinal, part) in bytes.chunks(PAGE).enumerate() {
        let id = Uuid::new_v4();
        let binding = json_string(
            &json!({"format":"android-private-workflow-registry-chunk/1","id":id,"owner":owner,"project":project,"object":object,"purpose":purpose,"ordinal":ordinal as i64,"size":part.len() as i64,"digest":hash(part),"key_id":key}),
        )?;
        let sealed = seal(keys, key, &binding, part)?;
        sqlx::query("INSERT INTO android_private_workflow_chunks(id,owner_id,project_id,object_id,purpose,ordinal,digest,size,sealed) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(id.to_string()).bind(owner.to_string()).bind(project.to_string()).bind(object.to_string()).bind(purpose).bind(ordinal as i64).bind(hash(part)).bind(part.len() as i64).bind(json_string(&sealed)?).execute(&mut **tx).await.map_err(storage_error)?;
    }
    Ok(())
}
pub(super) async fn read(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    object: Uuid,
    purpose: &str,
    size: i64,
    keys: &dyn KeyStore,
) -> Result<Zeroizing<Vec<u8>>> {
    if !(1..=3 * PAGE as i64).contains(&size) {
        return Err(Error::Forbidden);
    }
    let rows=sqlx::query("SELECT * FROM android_private_workflow_chunks WHERE owner_id=$1 AND project_id=$2 AND object_id=$3 AND purpose=$4 ORDER BY ordinal LIMIT 4").bind(owner.to_string()).bind(project.to_string()).bind(object.to_string()).bind(purpose).fetch_all(&mut **tx).await.map_err(storage_error)?;
    let mut bytes = Zeroizing::new(Vec::new());
    for (i, r) in rows.iter().enumerate() {
        if r.try_get::<i64, _>("ordinal").map_err(storage_error)? != i as i64 {
            return Err(Error::Forbidden);
        }
        let part = open(
            keys,
            r,
            &parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?,
        )?;
        if bytes.len().saturating_add(part.len()) > size as usize {
            return Err(Error::Forbidden);
        }
        bytes.extend_from_slice(&part);
    }
    if bytes.len() != size as usize {
        return Err(Error::Forbidden);
    }
    Ok(bytes)
}
pub(crate) async fn required_keys(tx: &mut Transaction<'_, Any>) -> Result<Vec<KeyId>> {
    let mut found = std::collections::BTreeMap::new();
    let mut last = String::new();
    loop {
        let rows=sqlx::query("SELECT id,sealed FROM android_private_workflow_chunks WHERE id>$1 ORDER BY id LIMIT 32").bind(&last).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for r in rows {
            last = r.try_get("id").map_err(storage_error)?;
            let v: Value = parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
            let key =
                serde_json::from_value::<KeyId>(v["key_id"].clone()).map_err(storage_error)?;
            key.validate().map_err(|_| Error::Forbidden)?;
            found.insert((key.instance, key.key), key);
        }
    }
    Ok(found.into_values().collect())
}
pub(crate) async fn preflight(tx: &mut Transaction<'_, Any>) -> Result<()> {
    let mut after = String::new();
    loop {
        let rows = sqlx::query("SELECT id,size,sealed FROM android_private_workflow_chunks WHERE id>$1 ORDER BY id LIMIT 32").bind(&after).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            return Ok(());
        }
        for row in rows {
            after = row.try_get("id").map_err(storage_error)?;
            let size: i64 = row.try_get("size").map_err(storage_error)?;
            let text: String = row.try_get("sealed").map_err(storage_error)?;
            if !(1..=PAGE as i64).contains(&size) || text.len() > 2 * PAGE {
                return Err(Error::Forbidden);
            }
            let envelope: Value = parse(&text)?;
            let key: KeyId =
                serde_json::from_value(envelope["key_id"].clone()).map_err(storage_error)?;
            key.validate().map_err(|_| Error::Forbidden)?;
            if envelope["format"] != 1
                || envelope["ciphertext"]
                    .as_str()
                    .is_none_or(|v| v.len() > 2 * PAGE)
                || envelope["nonce"].as_str().is_none_or(|v| v.len() > 64)
            {
                return Err(Error::Forbidden);
            }
        }
    }
}
pub(crate) async fn rewrap(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    let mut total = 0u64;
    let mut last = String::new();
    loop {
        let rows = sqlx::query(
            "SELECT * FROM android_private_workflow_chunks WHERE id>$1 ORDER BY id LIMIT 16",
        )
        .bind(&last)
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for r in rows {
            last = r.try_get("id").map_err(storage_error)?;
            let old: String = r.try_get("sealed").map_err(storage_error)?;
            let v: Value = parse(&old)?;
            let original: KeyId =
                serde_json::from_value(v["key_id"].clone()).map_err(storage_error)?;
            original.validate().map_err(|_| Error::Forbidden)?;
            target.validate().map_err(|_| Error::Forbidden)?;
            if original.instance != target.instance {
                return Err(Error::Forbidden);
            }
            let plain = open(keys, &r, &v)?;
            if original == target {
                continue;
            }
            let next = seal(keys, target, &aad(&r, target)?, &plain)?;
            if sqlx::query("UPDATE android_private_workflow_chunks SET sealed=$1 WHERE id=$2 AND owner_id=$3 AND project_id=$4 AND object_id=$5 AND purpose=$6 AND ordinal=$7 AND digest=$8 AND size=$9 AND sealed=$10").bind(json_string(&next)?).bind(&last).bind(uuid(&r,"owner_id")?.to_string()).bind(uuid(&r,"project_id")?.to_string()).bind(uuid(&r,"object_id")?.to_string()).bind(r.try_get::<String,_>("purpose").map_err(storage_error)?).bind(r.try_get::<i64,_>("ordinal").map_err(storage_error)?).bind(r.try_get::<String,_>("digest").map_err(storage_error)?).bind(r.try_get::<i64,_>("size").map_err(storage_error)?).bind(old).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
            total += 1;
        }
    }
    Ok(total)
}
