//! Read-only RFC2696 polling. Complete scans do not by themselves prove deletion.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use ldap3::{
    controls::{MakeCritical, PagedResults},
    Scope, SearchEntry, SearchOptions,
};
use tokio::sync::watch;
struct Claim {
    c: Current,
    cycle: Uuid,
    phase: String,
    cursor: Value,
    lease: Uuid,
    revision: i64,
    pages: i64,
    entries: i64,
}
pub async fn run(state: AppState, mut stop: watch::Receiver<bool>) {
    let mut timer = tokio::time::interval(Duration::from_secs(3));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {_ = stop.changed()=>{if *stop.borrow(){break;}},_ = timer.tick()=>{}}
        if *stop.borrow() {
            break;
        }
        let _ = resources::retain(&state).await;
        let mut ids: Vec<_> = state
            .directory
            .values()
            .filter(|c| c.kind == "ldap")
            .map(|c| c.id)
            .collect();
        ids.sort();
        for id in ids {
            if *stop.borrow() {
                break;
            }
            let claim = match claim(&state, id).await {
                Ok(Some(c)) => c,
                Ok(None) | Err(ApiError::Unauthorized(_)) => continue,
                Err(_) => {
                    tracing::warn!(connector=%id,"directory lease claim unavailable");
                    continue;
                }
            };
            let result = tokio::select! {_ = stop.changed()=>{Err(unavailable())},r=tokio::time::timeout(Duration::from_secs(40),tick(&state,&claim))=>r.unwrap_or_else(|_|Err(unavailable()))};
            if result.is_err() {
                let _ = fail(&state, &claim).await;
            }
        }
    }
}
async fn claim(state: &AppState, id: Uuid) -> Result<Option<Claim>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let c = current(state, &mut tx, id, false).await?;
    let initial = Uuid::new_v4();
    sqlx::query("INSERT INTO directory_sync_jobs(connector_id,cycle_id,registry_epoch,settings_epoch,phase) VALUES($1,$2,$3,$4,'users') ON CONFLICT(connector_id) DO NOTHING")
        .bind(id).bind(initial).bind(c.registry_epoch).bind(c.settings_epoch).execute(&mut *tx).await?;
    let row=sqlx::query("SELECT *, next_poll_at<=clock_timestamp() AND (lease_until IS NULL OR lease_until<=clock_timestamp()) AS claimable FROM directory_sync_jobs WHERE connector_id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    if !row.get::<bool, _>("claimable") {
        tx.commit().await?;
        return Ok(None);
    }
    let reset = row.get::<Uuid, _>("registry_epoch") != c.registry_epoch
        || row.get::<Uuid, _>("settings_epoch") != c.settings_epoch
        || row.get::<String, _>("phase") == "idle";
    let cycle = if reset {
        Uuid::new_v4()
    } else {
        row.get("cycle_id")
    };
    let phase = if reset {
        "users".to_owned()
    } else {
        row.get("phase")
    };
    let cursor = if reset || row.get::<Option<Vec<u8>>, _>("cursor_ciphertext").is_none() {
        json!({})
    } else {
        open(
            state,
            id,
            &format!("cursor:{cycle}:{phase}"),
            &sealed(&row, "cursor")?,
        )?
    };
    let revision = row.get::<i64, _>("revision") + 1;
    let lease = Uuid::new_v4();
    sqlx::query("UPDATE directory_sync_jobs SET revision=$2,cycle_id=$3,registry_epoch=$4,settings_epoch=$5,phase=$6,lease_id=$7,lease_until=clock_timestamp()+interval '90 seconds',completed_pages=CASE WHEN $8 THEN 0 ELSE completed_pages END,completed_entries=CASE WHEN $8 THEN 0 ELSE completed_entries END,cursor_ciphertext=CASE WHEN $8 THEN NULL ELSE cursor_ciphertext END,cursor_key=CASE WHEN $8 THEN NULL ELSE cursor_key END,cursor_nonce=CASE WHEN $8 THEN NULL ELSE cursor_nonce END,cursor_wrap_nonce=CASE WHEN $8 THEN NULL ELSE cursor_wrap_nonce END,updated_at=now() WHERE connector_id=$1")
        .bind(id).bind(revision).bind(cycle).bind(c.registry_epoch).bind(c.settings_epoch).bind(&phase).bind(lease).bind(reset).execute(&mut *tx).await?;
    let pages = if reset { 0 } else { row.get("completed_pages") };
    let entries = if reset {
        0
    } else {
        row.get("completed_entries")
    };
    tx.commit().await?;
    Ok(Some(Claim {
        c,
        cycle,
        phase,
        cursor,
        lease,
        revision,
        pages,
        entries,
    }))
}
async fn fenced(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    cl: &Claim,
) -> Result<(), ApiError> {
    recheck(state, tx, &cl.c).await?;
    let good:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM directory_sync_jobs WHERE connector_id=$1 AND cycle_id=$2 AND revision=$3 AND lease_id=$4 AND lease_until>clock_timestamp() AND registry_epoch=$5 AND settings_epoch=$6 AND phase=$7)")
        .bind(cl.c.connector.id).bind(cl.cycle).bind(cl.revision).bind(cl.lease).bind(cl.c.registry_epoch).bind(cl.c.settings_epoch).bind(&cl.phase).fetch_one(&mut **tx).await?;
    if !good {
        return Err(unavailable());
    }
    Ok(())
}
async fn secret(state: &AppState, cl: &Claim) -> Result<String, ApiError> {
    let mut tx = state.pool.begin().await?;
    fenced(state, &mut tx, cl).await?;
    let p = resources::credential(state, &mut tx, &cl.c).await?;
    tx.commit().await?;
    Ok(p)
}
async fn search(
    cl: &Claim,
    password: &str,
    k: &str,
    cookie: Vec<u8>,
    point: Option<&str>,
) -> Result<(Vec<SearchEntry>, Vec<u8>), ApiError> {
    let l = cl.c.connector.ldap.as_ref().ok_or_else(unavailable)?;
    let mut conn = tokio::time::timeout(
        Duration::from_secs(8),
        ldap_transport::connect(&cl.c.connector),
    )
    .await
    .map_err(|_| unavailable())??;
    let ldap = &mut conn.ldap;
    ldap.with_timeout(Duration::from_secs(10))
        .simple_bind(&l.bind_dn, password)
        .await
        .map_err(|_| unavailable())?
        .success()
        .map_err(|_| unavailable())?;
    let baseline = if k == "User" {
        &l.users_filter
    } else {
        &l.groups_filter
    };
    let filter = if let Some(external) = point {
        format!(
            "(&{baseline}({}={}))",
            l.subject_attribute,
            ldap3::ldap_escape(external)
        )
    } else {
        baseline.clone()
    };
    let mut attrs = vec![
        l.subject_attribute.as_str(),
        if k == "User" {
            l.username_attribute.as_str()
        } else {
            l.group_name_attribute.as_str()
        },
    ];
    if k == "Group" {
        attrs.push(&l.member_attribute);
    }
    if let Some(disabled) = &l.disabled_attribute {
        attrs.push(disabled);
    }
    ldap.with_search_options(
        SearchOptions::new()
            .timelimit(15)
            .sizelimit(if point.is_some() {
                2
            } else {
                l.page_size as i32 + 1
            }),
    )
    .with_timeout(Duration::from_secs(20));
    if point.is_none() {
        ldap.with_controls(
            PagedResults {
                size: l.page_size as i32,
                cookie,
            }
            .critical(),
        );
    }
    let mut stream = ldap
        .streaming_search(&l.base_dn, Scope::Subtree, &filter, attrs)
        .await
        .map_err(|_| unavailable())?;
    let mut rows = Vec::new();
    while let Some(entry) = stream.next().await.map_err(|_| unavailable())? {
        if entry.is_ref() || entry.is_intermediate() {
            return Err(unavailable());
        }
        let entry = std::panic::catch_unwind(|| SearchEntry::construct(entry))
            .map_err(|_| unavailable())?;
        let bytes = entry.dn.len()
            + entry
                .attrs
                .iter()
                .map(|(k, v)| k.len() + v.iter().map(String::len).sum::<usize>())
                .sum::<usize>()
            + entry
                .bin_attrs
                .iter()
                .map(|(k, v)| k.len() + v.iter().map(Vec::len).sum::<usize>())
                .sum::<usize>();
        if bytes > 524288
            || entry.attrs.len() > 16
            || !entry.bin_attrs.is_empty()
            || rows.len()
                >= if point.is_some() {
                    2
                } else {
                    l.page_size as usize
                }
        {
            return Err(unavailable());
        }
        rows.push(entry);
    }
    let result = stream.finish().await;
    if result.rc != 0 || !result.refs.is_empty() {
        return Err(unavailable());
    }
    let cookie = if point.is_some() {
        vec![]
    } else {
        let control = result
            .ctrls
            .iter()
            .find(|c| c.1.ctype == "1.2.840.113556.1.4.319")
            .ok_or_else(unavailable)?;
        let p = std::panic::catch_unwind(|| control.1.parse::<PagedResults>())
            .map_err(|_| unavailable())?;
        if p.cookie.len() > 65536 {
            return Err(unavailable());
        }
        p.cookie
    };
    Ok((rows, cookie))
}
fn attr<'a>(entry: &'a SearchEntry, name: &str) -> Result<&'a str, ApiError> {
    let values = entry
        .attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
        .ok_or_else(unavailable)?;
    if values.len() != 1
        || values[0].is_empty()
        || values[0].len() > 512
        || values[0].chars().any(char::is_control)
    {
        return Err(unavailable());
    }
    Ok(&values[0])
}
async fn input(
    tx: &mut Transaction<'_, Postgres>,
    cl: &Claim,
    k: &str,
    e: &SearchEntry,
) -> Result<resources::Input, ApiError> {
    let l = cl.c.connector.ldap.as_ref().ok_or_else(unavailable)?;
    let external = attr(e, &l.subject_attribute)?.to_owned();
    let name = attr(
        e,
        if k == "User" {
            &l.username_attribute
        } else {
            &l.group_name_attribute
        },
    )?
    .to_owned();
    let active = match (&l.disabled_attribute, &l.disabled_value) {
        (Some(a), Some(v)) => attr(e, a)? != v,
        _ => true,
    };
    let mut members = Vec::new();
    if k == "Group" {
        let dns = e
            .attrs
            .iter()
            .find(|(a, _)| a.eq_ignore_ascii_case(&l.member_attribute))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        if dns.len() > 1000 {
            return Err(unavailable());
        }
        for dn in dns {
            if dn.len() > 4096 {
                return Err(unavailable());
            }
            let users:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM directory_resources WHERE connector_id=$1 AND kind='User' AND dn_hash=$2 AND NOT deleted AND observed_scan=$3 ORDER BY id LIMIT 2")
                .bind(cl.c.connector.id).bind(hex::encode(Sha256::digest(dn.as_bytes()))).bind(cl.cycle).fetch_all(&mut **tx).await?;
            if users.len() != 1 {
                return Err(ApiError::Conflict("LDAP group member DN must resolve exactly to a User observed in this connector cycle".into()));
            }
            members.push(users[0]);
        }
    }
    let payload = if k == "User" {
        json!({"schemas":["urn:ietf:params:scim:schemas:core:2.0:User"],"externalId":external,"userName":name,"active":active})
    } else {
        json!({"schemas":["urn:ietf:params:scim:schemas:core:2.0:Group"],"externalId":external,"displayName":name,"members":members.iter().map(|id|json!({"value":id})).collect::<Vec<_>>()})
    };
    Ok(resources::Input {
        kind: k.into(),
        external: external.clone(),
        subject: (k == "User").then_some(external),
        name,
        active,
        deleted: false,
        payload,
        members,
        dn: Some(e.dn.clone()),
    })
}
async fn tick(state: &AppState, cl: &Claim) -> Result<(), ApiError> {
    if cl.pages >= 10000 || cl.entries >= 1_000_000 {
        return Err(ApiError::Quota(
            "LDAP cycle bound exceeded; no incomplete-scan deletion is permitted".into(),
        ));
    }
    let password = secret(state, cl).await?;
    let k = if matches!(cl.phase.as_str(), "users" | "reconcile_users") {
        "User"
    } else {
        "Group"
    };
    let mut nextphase = cl.phase.clone();
    let mut nextcursor = json!({});
    let mut inputs = Vec::new();
    if cl.phase.starts_with("reconcile_") {
        let after = cl.cursor["after"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
            .unwrap_or(Uuid::nil());
        let mut tx = state.pool.begin().await?;
        fenced(state, &mut tx, cl).await?;
        let rows=sqlx::query("SELECT * FROM directory_resources WHERE connector_id=$1 AND kind=$2 AND NOT deleted AND observed_scan IS DISTINCT FROM $3 AND id>$4 ORDER BY id LIMIT 1")
            .bind(cl.c.connector.id).bind(k).bind(cl.cycle).bind(after).fetch_all(&mut *tx).await?;
        if let Some(row) = rows.first() {
            let external: String = row.get("external_key");
            let original = resources::payload(state, row, cl.c.connector.id).await?;
            let id: Uuid = row.get("id");
            let revision: i64 = row.get("revision");
            tx.commit().await?;
            let (found, _) = search(cl, &password, k, vec![], Some(&external)).await?;
            if found.len() > 1 {
                return Err(unavailable());
            }
            let mut tx = state.pool.begin().await?;
            fenced(state, &mut tx, cl).await?;
            let i = if let Some(e) = found.first() {
                input(&mut tx, cl, k, e).await?
            } else {
                let mut i = scim::input(k, original)?;
                i.deleted = true;
                i.active = false;
                i
            };
            inputs.push((Some(id), Some(revision), i));
            nextcursor = json!({"after":id});
            tx.commit().await?;
        } else {
            nextphase = if k == "User" {
                "reconcile_groups"
            } else {
                "idle"
            }
            .into();
            tx.commit().await?;
        }
    } else {
        let cookie = cl.cursor["cookie"]
            .as_str()
            .map(|s| STANDARD.decode(s).map_err(|_| unavailable()))
            .transpose()?
            .unwrap_or_default();
        if cookie.len() > 65536 {
            return Err(unavailable());
        }
        let (rows, cookie) = search(cl, &password, k, cookie, None).await?;
        let mut tx = state.pool.begin().await?;
        fenced(state, &mut tx, cl).await?;
        for e in rows {
            inputs.push((None, None, input(&mut tx, cl, k, &e).await?));
        }
        if cookie.is_empty() {
            nextphase = if k == "User" {
                "groups"
            } else {
                "reconcile_users"
            }
            .into();
        } else {
            nextcursor = json!({"cookie":STANDARD.encode(cookie)});
        }
        tx.commit().await?;
    }
    let mut tx = state.pool.begin().await?;
    fenced(state, &mut tx, cl).await?;
    for (id, revision, i) in &inputs {
        let key = format!(
            "ldap:{}:{}:{}:{}",
            cl.cycle,
            cl.phase,
            cl.pages,
            sha(&json!([i.kind, i.external]))
        );
        let hash = sha(&json!([
            cl.c.stamp(),
            cl.cycle,
            cl.phase,
            cl.pages,
            i.payload,
            i.deleted
        ]));
        if resources::replay(state, &mut tx, &cl.c, &key, &hash)
            .await?
            .is_none()
        {
            let result =
                resources::upsert(state, &mut tx, &cl.c, *id, *revision, i, Some(cl.cycle)).await?;
            let id = result["id"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .ok_or_else(unavailable)?;
            resources::journal(state, &mut tx, &cl.c, &key, &hash, id, &result).await?;
        }
    }
    fenced(state, &mut tx, cl).await?;
    let encrypted = seal(
        state,
        cl.c.connector.id,
        &format!("cursor:{}:{nextphase}", cl.cycle),
        &nextcursor,
    )?;
    let seconds = if nextphase == "idle" {
        cl.c.connector.ldap.as_ref().unwrap().poll_seconds as i64
    } else {
        0
    };
    sqlx::query("UPDATE directory_sync_jobs SET revision=revision+1,phase=$2,cursor_ciphertext=$3,cursor_key=$4,cursor_nonce=$5,cursor_wrap_nonce=$6,completed_pages=completed_pages+1,completed_entries=completed_entries+$7,lease_id=NULL,lease_until=NULL,next_poll_at=clock_timestamp()+make_interval(secs=>$8::DOUBLE PRECISION),failures=0,last_error_code=NULL,last_success_at=CASE WHEN $2='idle' THEN clock_timestamp() ELSE last_success_at END,updated_at=now() WHERE connector_id=$1 AND revision=$9 AND lease_id=$10")
        .bind(cl.c.connector.id).bind(&nextphase).bind(encrypted.ciphertext).bind(encrypted.encrypted_data_key).bind(encrypted.nonce.as_slice()).bind(encrypted.wrap_nonce.as_slice()).bind(inputs.len() as i64).bind(seconds as f64).bind(cl.revision).bind(cl.lease).execute(&mut *tx).await?;
    if nextphase == "idle" {
        append_audit(&mut tx,cl.c.connector.org,Uuid::nil(),"directory.ldap_cycle_completed",cl.c.connector.id,json!({"cycle_id":cl.cycle,"source":cl.c.stamp(),"pages":cl.pages+1,"entries":cl.entries+inputs.len() as i64})).await?;
    }
    tx.commit().await?;
    Ok(())
}
async fn fail(state: &AppState, cl: &Claim) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await?;
    // Failure never deletes a source identity, and resets uncertain cookies to
    // a new complete scan. A stale worker cannot release another worker's lease.
    sqlx::query("UPDATE directory_sync_jobs SET revision=revision+1,phase='users',cycle_id=$4,cursor_ciphertext=NULL,cursor_key=NULL,cursor_nonce=NULL,cursor_wrap_nonce=NULL,completed_pages=0,completed_entries=0,lease_id=NULL,lease_until=NULL,failures=least(failures+1,10),last_error_code='ldap_source_unavailable_or_changed',next_poll_at=clock_timestamp()+make_interval(secs=>least(3600,30*power(2,least(failures,7)))::DOUBLE PRECISION),updated_at=now() WHERE connector_id=$1 AND revision=$2 AND lease_id=$3")
        .bind(cl.c.connector.id).bind(cl.revision).bind(cl.lease).bind(Uuid::new_v4()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
