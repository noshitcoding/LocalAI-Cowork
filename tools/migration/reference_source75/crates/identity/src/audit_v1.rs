//! V1 0.3.12's exact canonical HMAC format. Original JSONL never becomes runtime
//! authority and no event contents cross this metadata-only boundary.
use crate::{
    IdentityError, Result,
    audit::{self, AuditAuthority},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cowork_contracts::*;
use cowork_secrets::NativeV1AuditKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Any, Row, Transaction};
use std::collections::BTreeSet;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Integrity {
    version: u32,
    sequence: u64,
    previous_mac: Option<String>,
    mac: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    timestamp: String,
    area: String,
    action: String,
    details: Value,
    integrity: Integrity,
}
#[derive(Serialize)]
struct Unsigned<'a> {
    timestamp: &'a str,
    area: &'a str,
    action: &'a str,
    details: &'a Value,
    integrity: UnsignedIntegrity<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsignedIntegrity<'a> {
    version: u32,
    sequence: u64,
    previous_mac: Option<&'a str>,
}
fn mac(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || value
            .bytes()
            .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).ok()?;
    }
    if value
        .bytes()
        .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
    {
        return None;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line(sequence: u64, previous: Option<String>) -> String {
        let mut r = Record {
            timestamp: "2026-10-01T00:00:00Z".into(),
            area: "fixture".into(),
            action: "fixture_record".into(),
            details: serde_json::json!({"redacted":"fixture"}),
            integrity: Integrity {
                version: 1,
                sequence,
                previous_mac: previous,
                mac: String::new(),
            },
        };
        let bytes = serde_json::to_vec(&Unsigned {
            timestamp: &r.timestamp,
            area: &r.area,
            action: &r.action,
            details: &r.details,
            integrity: UnsignedIntegrity {
                version: 1,
                sequence,
                previous_mac: r.integrity.previous_mac.as_deref(),
            },
        })
        .unwrap();
        r.integrity.mac = ring::hmac::sign(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &[4u8; 32]),
            &bytes,
        )
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
        serde_json::to_string(&r).unwrap()
    }
    fn check(bytes: &[u8], mac: &[u8]) -> bool {
        ring::hmac::verify(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &[4u8; 32]),
            bytes,
            mac,
        )
        .is_ok()
    }
    #[test]
    fn exact_v1_rotated_segments_and_partial_anchors_are_distinguished() {
        let first = line(1, None);
        let previous = serde_json::from_str::<Record>(&first)
            .unwrap()
            .integrity
            .mac;
        let second = line(2, Some(previous));
        let files = vec![
            format!("{first}\n").into_bytes(),
            format!("{second}\n").into_bytes(),
        ];
        let v = inspect(&files, check).unwrap();
        assert!(v.valid && v.chain_complete);
        assert_eq!(v.signed_events, 2);
        let partial = inspect(&[files[1].clone()], check).unwrap();
        assert!(partial.valid);
        assert!(!partial.chain_complete);
        assert!(!inspect(&[first.as_bytes().to_vec()], check).unwrap().valid);
        let altered = files[0]
            .iter()
            .copied()
            .map(|b| if b == b'x' { b'y' } else { b })
            .collect();
        assert!(!inspect(&[altered], check).unwrap().valid);
        assert!(mac(&format!("€{}", "a".repeat(61))).is_none());
    }
}
fn inspect(files: &[Vec<u8>], check: impl Fn(&[u8], &[u8]) -> bool) -> Result<AuditV1Assessment> {
    let mut out = AuditV1Assessment {
        valid: true,
        signed_events: 0,
        legacy_events: 0,
        first_sequence: None,
        last_sequence: None,
        chain_complete: false,
        first_problem_line: None,
        error_code: None,
    };
    let mut line_number = 0i64;
    let mut previous = None::<String>;
    let mut last = None::<u64>;
    macro_rules! fail {
        ($code:literal) => {{
            out.valid = false;
            out.error_code = Some($code.into());
            out.first_problem_line = Some(line_number.max(1));
            return Ok(out);
        }};
    }
    for bytes in files {
        if bytes.is_empty() {
            continue;
        }
        if bytes.last() != Some(&b'\n') {
            fail!("audit_v1_truncated_record");
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            fail!("audit_v1_invalid_encoding");
        };
        for line in text.lines() {
            line_number += 1;
            if line_number > 100_000 {
                return Err(IdentityError::QuotaExceeded);
            }
            if line.is_empty() || line.len() > 1024 * 1024 {
                fail!("audit_v1_invalid_record");
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                fail!("audit_v1_malformed_record");
            };
            let Some(object) = value.as_object() else {
                fail!("audit_v1_malformed_record");
            };
            if !object.contains_key("integrity") {
                if previous.is_some() {
                    fail!("audit_v1_legacy_after_signed");
                }
                if !["timestamp", "area", "action"]
                    .iter()
                    .all(|k| object.get(*k).and_then(Value::as_str).is_some())
                {
                    fail!("audit_v1_malformed_legacy_record");
                }
                out.legacy_events += 1;
                continue;
            }
            let Ok(record) = serde_json::from_str::<Record>(line) else {
                fail!("audit_v1_invalid_integrity");
            };
            if serde_json::to_string(&record).map_err(|_| IdentityError::Corrupt)? != line {
                fail!("audit_v1_noncanonical_record");
            }
            let i = &record.integrity;
            if i.version != 1
                || i.sequence == 0
                || (i.sequence == 1 && i.previous_mac.is_some())
                || (i.sequence > 1 && i.previous_mac.is_none())
            {
                fail!("audit_v1_sequence_anchor");
            }
            let Some(signature) = mac(&i.mac) else {
                fail!("audit_v1_invalid_mac");
            };
            if i.previous_mac.as_deref().is_some_and(|s| mac(s).is_none()) {
                fail!("audit_v1_invalid_mac");
            }
            let canonical = serde_json::to_vec(&Unsigned {
                timestamp: &record.timestamp,
                area: &record.area,
                action: &record.action,
                details: &record.details,
                integrity: UnsignedIntegrity {
                    version: i.version,
                    sequence: i.sequence,
                    previous_mac: i.previous_mac.as_deref(),
                },
            })
            .map_err(|_| IdentityError::Corrupt)?;
            if !check(&canonical, &signature) {
                fail!("audit_v1_mac_mismatch");
            }
            if let Some(old) = &previous {
                if last.and_then(|n| n.checked_add(1)) != Some(i.sequence)
                    || i.previous_mac.as_ref() != Some(old)
                {
                    fail!("audit_v1_sequence_gap");
                }
            }
            if out.first_sequence.is_none() {
                out.first_sequence = Some(i.sequence.to_string());
            }
            out.signed_events += 1;
            out.last_sequence = Some(i.sequence.to_string());
            last = Some(i.sequence);
            previous = Some(i.mac.clone());
        }
    }
    out.chain_complete = out.signed_events > 0
        && out.legacy_events == 0
        && out.first_sequence.as_deref() == Some("1");
    // Unsigned-only history remains unverified even when structurally valid.
    if out.signed_events == 0 {
        out.valid = false;
        out.error_code = Some("audit_v1_no_signed_records".into());
    }
    Ok(out)
}
pub async fn capture(
    tx: &mut Transaction<'_, Any>,
    authority: &AuditAuthority,
    input: VerifyAuditV1,
    observed_at: i64,
) -> Result<AuditV1Verification> {
    if input.request_id.is_nil() || input.archives.is_empty() || input.archives.len() > 4 {
        return Err(IdentityError::Validation);
    }
    let mut ids = BTreeSet::new();
    for a in &input.archives {
        if a.request_id.is_nil()
            || !ids.insert(a.request_id)
            || a.content_sha256.len() != 64
            || a.content_sha256
                .bytes()
                .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
        {
            return Err(IdentityError::Validation);
        }
    }
    let request_sha = audit::hash(
        serde_json::to_vec(&input)
            .map_err(|_| IdentityError::Corrupt)?
            .as_slice(),
    );
    // Same audit writer fence as ordinary events and archive preservation.
    audit::writer_fence(tx, "personal").await?;
    let cached=if let Some(row)=sqlx::query("SELECT request_sha256,payload,payload_sha256 FROM audit_v1_receipts WHERE scope='personal' AND request_id=$1").bind(input.request_id.to_string()).fetch_optional(&mut **tx).await?{
        if row.try_get::<String,_>("request_sha256")?!=request_sha{return Err(IdentityError::Conflict);}let raw:String=row.try_get("payload")?;if audit::hash(raw.as_bytes())!=row.try_get::<String,_>("payload_sha256")?{return Err(IdentityError::Corrupt);}let out:AuditV1Verification=serde_json::from_str(&raw).map_err(|_|IdentityError::Corrupt)?;
        if out.request_id!=input.request_id||out.scope!="personal"||serde_json::to_value(&out.archives).map_err(|_|IdentityError::Corrupt)?!=serde_json::to_value(&input.archives).map_err(|_|IdentityError::Corrupt)?{return Err(IdentityError::Corrupt);}let mut unsigned=out.clone();unsigned.public_key.clear();unsigned.public_key_sha256.clear();unsigned.signature.clear();audit::verify_metadata("cowork:audit:v1-hmac-verification:v1",&out.public_key,&out.public_key_sha256,&out.signature,&unsigned)?;Some(out)
    }else{None};
    let mut files = Vec::new();
    let mut total = 0usize;
    for a in &input.archives {
        let raw = audit::history(tx, "personal", a.request_id).await?;
        let bytes = URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| IdentityError::Corrupt)?;
        if audit::hash(&bytes) != a.content_sha256 {
            return Err(IdentityError::Conflict);
        }
        total = total
            .checked_add(bytes.len())
            .ok_or(IdentityError::QuotaExceeded)?;
        if total > 64 * 1024 * 1024 {
            return Err(IdentityError::QuotaExceeded);
        }
        files.push(bytes);
    }
    if let Some(out) = cached {
        return Ok(out);
    }
    crate::audit_origin::admission(tx, authority).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_v1_receipts WHERE scope='personal'")
            .fetch_one(&mut **tx)
            .await?;
    if count >= 4096 {
        return Err(IdentityError::QuotaExceeded);
    }
    let key = NativeV1AuditKey::load().map_err(|_| IdentityError::NativeAuditKeyUnavailable)?;
    let assessment = inspect(&files, |data, mac| key.verify(data, mac))?;
    let mut out = AuditV1Verification {
        format: "cowork.audit.v1-hmac.verification.v1".into(),
        request_id: input.request_id,
        scope: "personal".into(),
        captured_at: observed_at,
        archives: input.archives,
        assessment,
        key_source: "windows-credential-manager-v1-read-only".into(),
        public_key: String::new(),
        public_key_sha256: String::new(),
        signature: String::new(),
    };
    let (public, pin, signature) =
        authority.sign_metadata("cowork:audit:v1-hmac-verification:v1", &out)?;
    out.public_key = public;
    out.public_key_sha256 = pin;
    out.signature = signature;
    let raw = serde_json::to_string(&out).map_err(|_| IdentityError::Corrupt)?;
    sqlx::query("INSERT INTO audit_v1_receipts(scope,request_id,request_sha256,payload,payload_sha256) VALUES('personal',$1,$2,$3,$4)").bind(out.request_id.to_string()).bind(request_sha).bind(&raw).bind(audit::hash(raw.as_bytes())).execute(&mut **tx).await?;
    audit::append(
        tx,
        "personal",
        audit::MetadataEvent {
            event_id: &format!("v1-verification:{}", out.request_id),
            action: "history_v1_hmac_checked",
            actor_id: None,
            target_id: None,
            observed_at,
            origin: AuditOrigin::PreservedArchive,
            source_sha256: Some(audit::hash(raw.as_bytes())),
        },
    )
    .await?;
    Ok(out)
}
