//! Strict enrolled SPKI parsing; the witness can never select its own key.
use super::*;
use base64::engine::general_purpose::STANDARD;
fn der<'a>(input: &mut &'a [u8], tag: u8) -> Result<&'a [u8]> {
    if input.len() < 2 || input[0] != tag {
        return Err(invalid());
    }
    let mut n = input[1] as usize;
    let mut head = 2;
    if n & 128 != 0 {
        let count = n & 127;
        if count == 0 || count > 4 || input.len() < 2 + count || input[2] == 0 {
            return Err(invalid());
        }
        n = 0;
        for b in &input[2..2 + count] {
            n = n
                .checked_mul(256)
                .and_then(|v| v.checked_add(*b as usize))
                .ok_or_else(invalid)?;
        }
        if n < 128 {
            return Err(invalid());
        }
        head += count;
    }
    let end = head.checked_add(n).ok_or_else(invalid)?;
    if end > input.len() {
        return Err(invalid());
    }
    let result = &input[head..end];
    *input = &input[end..];
    Ok(result)
}
fn positive(input: &[u8]) -> Result<&[u8]> {
    if input.is_empty() || input[0] & 128 != 0 {
        return Err(invalid());
    }
    if input[0] == 0 {
        if input.len() < 2 || input[1] & 128 == 0 {
            return Err(invalid());
        }
        Ok(&input[1..])
    } else {
        Ok(input)
    }
}
pub(super) fn public_components(raw: &[u8]) -> Result<(&[u8], &[u8])> {
    if raw.len() > 2048 {
        return Err(invalid());
    }
    let mut outer = raw;
    let mut seq = der(&mut outer, 0x30)?;
    if !outer.is_empty() {
        return Err(invalid());
    }
    let mut alg = der(&mut seq, 0x30)?;
    if der(&mut alg, 6)? != [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 1]
        || !der(&mut alg, 5)?.is_empty()
        || !alg.is_empty()
    {
        return Err(invalid());
    }
    let bits = der(&mut seq, 3)?;
    if !seq.is_empty() || bits.first() != Some(&0) {
        return Err(invalid());
    }
    let mut keys = &bits[1..];
    let mut rsa = der(&mut keys, 0x30)?;
    if !keys.is_empty() {
        return Err(invalid());
    }
    let n = positive(der(&mut rsa, 2)?)?;
    let e = positive(der(&mut rsa, 2)?)?;
    if !rsa.is_empty() || n.is_empty() || e.is_empty() || e.len() > 4 {
        return Err(invalid());
    }
    let width = n.len() * 8 - n[0].leading_zeros() as usize;
    let exponent = e.iter().fold(0u64, |v, b| v * 256 + u64::from(*b));
    if !(3072..=8192).contains(&width) || exponent < 3 || exponent % 2 == 0 {
        return Err(invalid());
    }
    Ok((n, e))
}
pub(super) fn base64(raw: &str, limit: usize) -> Result<Vec<u8>> {
    if raw.len() > limit.saturating_mul(4).saturating_add(8) {
        return Err(invalid());
    }
    let bytes = STANDARD.decode(raw).map_err(|_| invalid())?;
    if bytes.len() > limit || STANDARD.encode(&bytes) != raw {
        return Err(invalid());
    }
    Ok(bytes)
}
pub(super) fn verify(
    profile: &OperatorHostProfile,
    intent: &Value,
    witness: &Value,
) -> Result<Uuid> {
    if witness.as_object().is_none_or(|m| m.len() != 3)
        || witness["public_key_sha256"].as_str() != Some(profile.public_key_sha256.as_str())
    {
        return Err(Error::Forbidden);
    }
    let p = &witness["payload"];
    let fields = [
        "format",
        "intent",
        "intent_sha256",
        "plan_sha256",
        "retirement_id",
        "deletion_witness_sha256",
        "original_allocation_sha256",
        "controller_source_pins",
        "controller_capacity_revision",
        "physical_retirement_verified",
        "observed_at",
    ];
    if p.as_object()
        .is_none_or(|m| m.len() != fields.len() || fields.iter().any(|f| !m.contains_key(*f)))
        || p["format"] != "cowork-physical-host-retirement-v1"
        || &p["intent"] != intent
        || p["intent_sha256"] != digest(intent)?
        || p["physical_retirement_verified"] != true
        || p["original_allocation_sha256"] != profile.allocation_sha256
        || p["controller_source_pins"] != profile.controller_source_pins
        || p["controller_capacity_revision"]
            .as_u64()
            .is_none_or(|v| v == 0)
    {
        return Err(Error::Forbidden);
    }
    for field in [
        "intent_sha256",
        "plan_sha256",
        "deletion_witness_sha256",
        "original_allocation_sha256",
    ] {
        if !sha(p[field].as_str().ok_or_else(invalid)?) {
            return Err(invalid());
        }
    }
    let stamp = p["observed_at"].as_str().ok_or_else(invalid)?;
    if stamp.len() > 64
        || chrono::DateTime::parse_from_rfc3339(stamp)
            .map_err(|_| invalid())?
            .offset()
            .local_minus_utc()
            != 0
    {
        return Err(invalid());
    }
    let retirement = uuid_value(&p["retirement_id"])?;
    let public = base64(&profile.public_key_der, 2048)?;
    let (n, e) = public_components(&public)?;
    let signature = base64(witness["signature"].as_str().ok_or_else(invalid)?, 1024)?;
    ring::signature::RsaPublicKeyComponents { n, e }
        .verify(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            &serde_json::to_vec(p).map_err(storage_error)?,
            &signature,
        )
        .map_err(|_| Error::Forbidden)?;
    Ok(retirement)
}

pub(super) fn verify_signature(
    profile: &OperatorHostProfile,
    payload: &Value,
    signature: &str,
) -> Result<()> {
    let public = base64(&profile.public_key_der, 2048)?;
    if bytes_sha(&public) != profile.public_key_sha256 {
        return Err(Error::Forbidden);
    }
    let (n, e) = public_components(&public)?;
    let bytes = base64(signature, 1024)?;
    ring::signature::RsaPublicKeyComponents { n, e }
        .verify(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            &serde_json::to_vec(payload).map_err(storage_error)?,
            &bytes,
        )
        .map_err(|_| Error::Forbidden)
}
