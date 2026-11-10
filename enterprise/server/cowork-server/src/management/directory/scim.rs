//! Bounded RFC7643/7644 surface; no SCIM cookie or tenant-token fallback.
use super::*;
use axum::{
    body::Bytes,
    extract::Query,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use subtle::ConstantTimeEq;
#[derive(Debug)]
pub(super) struct Error(ApiError);
impl From<ApiError> for Error {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, kind, msg) = match self.0 {
            ApiError::Unauthorized(_) => (
                StatusCode::UNAUTHORIZED,
                "",
                "directory authorization unavailable",
            ),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "", "directory resource not found"),
            ApiError::Conflict(_) => (
                StatusCode::CONFLICT,
                "uniqueness",
                "original resource/version or operation conflicts; inspect before retry",
            ),
            ApiError::Unprocessable(_) | ApiError::BadRequest(_) | ApiError::Json(_) => (
                StatusCode::BAD_REQUEST,
                "invalidValue",
                "invalid or unsupported SCIM value/filter/patch",
            ),
            ApiError::Quota(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "",
                "SCIM resource bound exceeded",
            ),
            _ => (
                StatusCode::SERVICE_UNAVAILABLE,
                "",
                "directory service unavailable",
            ),
        };
        let mut body = json!({"schemas":["urn:ietf:params:scim:api:messages:2.0:Error"],"status":status.as_u16().to_string(),"detail":msg});
        if !kind.is_empty() {
            body["scimType"] = json!(kind);
        }
        response(status, body)
    }
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{connector}/ServiceProviderConfig", get(discovery))
        .route("/{connector}/ResourceTypes", get(types))
        .route("/{connector}/Schemas", get(schemas))
        .route("/{connector}/{kind}", get(list).post(create))
        .route(
            "/{connector}/{kind}/{resource}",
            get(get_one).put(replace).patch(update).delete(remove),
        )
        .layer(axum::extract::DefaultBodyLimit::max(524_288))
}
fn response(status: StatusCode, v: Value) -> Response {
    let mut r = (status, Json(v)).into_response();
    r.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("application/scim+json"),
    );
    r
}
async fn authorize(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    headers: &HeaderMap,
) -> Result<Current, ApiError> {
    let c = current(state, tx, id, false).await?;
    if c.connector.kind != "scim" {
        return Err(unavailable());
    }
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|h| h.len() <= 8192)
        .ok_or_else(unavailable)?;
    let actual = Sha256::digest(token.as_bytes());
    let credential = resources::credential(state, tx, &c).await?;
    let expected = Sha256::digest(credential.as_bytes());
    if !bool::from(actual.as_slice().ct_eq(expected.as_slice())) {
        return Err(unavailable());
    }
    Ok(c)
}
async fn discovery(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    authorize(&state, &mut tx, id, &headers).await?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(response(
        StatusCode::OK,
        json!({"schemas":["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],"patch":{"supported":true},"bulk":{"supported":false,"maxOperations":0,"maxPayloadSize":0},"filter":{"supported":true,"maxResults":100},"changePassword":{"supported":false},"sort":{"supported":false},"etag":{"supported":true},"authenticationSchemes":[{"type":"oauthbearertoken","name":"Original connector bearer","description":"Private operator token; current connector, SSO and organisation policy required","primary":true}]}),
    ))
}
async fn types(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    authorize(&state, &mut tx, id, &headers).await?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(response(
        StatusCode::OK,
        json!({"schemas":["urn:ietf:params:scim:api:messages:2.0:ListResponse"],"totalResults":2,"startIndex":1,"itemsPerPage":2,"Resources":[{"schemas":["urn:ietf:params:scim:schemas:core:2.0:ResourceType"],"id":"User","name":"User","endpoint":"/Users","schema":"urn:ietf:params:scim:schemas:core:2.0:User"},{"schemas":["urn:ietf:params:scim:schemas:core:2.0:ResourceType"],"id":"Group","name":"Group","endpoint":"/Groups","schema":"urn:ietf:params:scim:schemas:core:2.0:Group"}]}),
    ))
}
async fn schemas(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    authorize(&state, &mut tx, id, &headers).await?;
    tx.commit().await.map_err(ApiError::from)?;
    let attrs = |group: bool| {
        let mut a = vec![
            json!({"name":"externalId","type":"string","multiValued":false,"required":true,"mutability":"immutable","returned":"default","uniqueness":"server"}),
        ];
        a.push(json!({"name":if group{"displayName"}else{"userName"},"type":"string","multiValued":false,"required":true,"mutability":"readWrite","returned":"default","uniqueness":"none"}));
        if group {
            a.push(json!({"name":"members","type":"complex","multiValued":true,"required":false,"mutability":"readWrite","returned":"default","uniqueness":"none","subAttributes":[{"name":"value","type":"string","multiValued":false,"required":true,"mutability":"immutable","returned":"default","uniqueness":"none"}]}));
        } else {
            a.push(json!({"name":"active","type":"boolean","multiValued":false,"required":false,"mutability":"readWrite","returned":"default","uniqueness":"none"}));
        }
        a
    };
    Ok(response(
        StatusCode::OK,
        json!({"schemas":["urn:ietf:params:scim:api:messages:2.0:ListResponse"],"totalResults":2,"Resources":[{"schemas":["urn:ietf:params:scim:schemas:core:2.0:Schema"],"id":"urn:ietf:params:scim:schemas:core:2.0:User","name":"User","attributes":attrs(false)},{"schemas":["urn:ietf:params:scim:schemas:core:2.0:Schema"],"id":"urn:ietf:params:scim:schemas:core:2.0:Group","name":"Group","attributes":attrs(true)}],"startIndex":1,"itemsPerPage":2}),
    ))
}
fn kind(raw: &str) -> Result<&str, ApiError> {
    match raw {
        "Users" => Ok("User"),
        "Groups" => Ok("Group"),
        _ => Err(ApiError::NotFound("SCIM resource type not found".into())),
    }
}
fn etag(h: &HeaderMap) -> Result<i64, ApiError> {
    let s = h
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| invalid("If-Match is required"))?;
    s.strip_prefix("W/\"")
        .and_then(|v| v.strip_suffix('"'))
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .ok_or_else(|| invalid("invalid If-Match"))
}
pub(super) fn input(k: &str, v: Value) -> Result<resources::Input, ApiError> {
    let object = v
        .as_object()
        .ok_or_else(|| invalid("SCIM resource must be an object"))?;
    if object.len() > 32 || object.contains_key("password") {
        return Err(invalid("password provisioning is unsupported"));
    }
    let external = v["externalId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
        .ok_or_else(|| invalid("original externalId is required"))?
        .to_owned();
    let name = v[if k == "User" {
        "userName"
    } else {
        "displayName"
    }]
    .as_str()
    .filter(|s| !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
    .ok_or_else(|| invalid("resource name is required"))?
    .to_owned();
    let active = match v.get("active") {
        None => true,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| invalid("active must be boolean"))?,
    };
    let members = if k == "Group" {
        match v.get("members") {
            None => vec![],
            Some(m) => m
                .as_array()
                .ok_or_else(|| invalid("members must be an array"))?
                .iter()
                .map(|v| {
                    v["value"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| {
                            invalid("member value must be a same-connector resource UUID")
                        })
                })
                .collect::<Result<Vec<Uuid>, ApiError>>()?,
        }
    } else {
        vec![]
    };
    let mut payload = v.clone();
    payload.as_object_mut().unwrap().remove("id");
    payload.as_object_mut().unwrap().remove("meta");
    payload["schemas"] = json!([format!("urn:ietf:params:scim:schemas:core:2.0:{k}")]);
    Ok(resources::Input {
        kind: k.into(),
        subject: (k == "User").then(|| external.clone()),
        external,
        name,
        active,
        deleted: false,
        payload,
        members,
        dn: None,
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Page {
    start_index: Option<i64>,
    count: Option<i64>,
    filter: Option<String>,
}
async fn list(
    State(state): State<AppState>,
    Path((id, k)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Query(q): Query<Page>,
) -> Result<Response, Error> {
    let k = kind(&k)?;
    let start = q.start_index.unwrap_or(1);
    let count = q.count.unwrap_or(100);
    if start < 1 || !(0..=100).contains(&count) || start > 1_000_000 {
        return Err(invalid("invalid page bound").into());
    }
    let mut external = None;
    let mut name = None;
    if let Some(f) = q.filter {
        if f.len() > 1024 {
            return Err(invalid("filter too long").into());
        }
        let (field, value) = f
            .split_once(" eq ")
            .ok_or_else(|| invalid("only exact equality filters are supported"))?;
        let value: String = serde_json::from_str(value).map_err(ApiError::from)?;
        match field {
            "externalId" => external = Some(value),
            "userName" if k == "User" => name = Some(hex::encode(Sha256::digest(value.as_bytes()))),
            "displayName" if k == "Group" => {
                name = Some(hex::encode(Sha256::digest(value.as_bytes())))
            }
            _ => return Err(invalid("unsupported filter").into()),
        }
    }
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    authorize(&state, &mut tx, id, &headers).await?;
    let total:i64=sqlx::query_scalar("SELECT count(*) FROM directory_resources WHERE connector_id=$1 AND kind=$2 AND NOT deleted AND ($3::TEXT IS NULL OR external_key=$3) AND ($4::TEXT IS NULL OR name_hash=$4)").bind(id).bind(k).bind(&external).bind(&name).fetch_one(&mut *tx).await.map_err(ApiError::from)?;
    let rows=sqlx::query("SELECT * FROM directory_resources WHERE connector_id=$1 AND kind=$2 AND NOT deleted AND ($3::TEXT IS NULL OR external_key=$3) AND ($4::TEXT IS NULL OR name_hash=$4) ORDER BY id OFFSET $5 LIMIT $6")
        .bind(id).bind(k).bind(external).bind(name).bind(start-1).bind(count).fetch_all(&mut *tx).await.map_err(ApiError::from)?;
    let mut result = Vec::new();
    for r in rows {
        result.push(render(&state, &r, id).await?);
    }
    tx.commit().await.map_err(ApiError::from)?;
    Ok(response(
        StatusCode::OK,
        json!({"schemas":["urn:ietf:params:scim:api:messages:2.0:ListResponse"],"totalResults":total,"startIndex":start,"itemsPerPage":result.len(),"Resources":result}),
    ))
}
async fn render(state: &AppState, r: &sqlx::postgres::PgRow, id: Uuid) -> Result<Value, ApiError> {
    let mut v = resources::payload(state, r, id).await?;
    v["id"] = json!(r.get::<Uuid, _>("id"));
    v["meta"] = json!({"resourceType":r.get::<String,_>("kind"),"version":format!("W/\"{}\"",r.get::<i64,_>("revision")),"created":r.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),"lastModified":r.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")});
    Ok(v)
}
async fn get_one(
    State(state): State<AppState>,
    Path((id, k, rid)): Path<(Uuid, String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    let k = kind(&k)?;
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    authorize(&state, &mut tx, id, &headers).await?;
    let r=sqlx::query("SELECT * FROM directory_resources WHERE connector_id=$1 AND id=$2 AND kind=$3 AND NOT deleted").bind(id).bind(rid).bind(k).fetch_optional(&mut *tx).await.map_err(ApiError::from)?.ok_or_else(||ApiError::NotFound("resource not found".into()))?;
    let v = render(&state, &r, id).await?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(resource_response(StatusCode::OK, v, id, &k))
}
fn resource_response(status: StatusCode, v: Value, id: Uuid, k: &str) -> Response {
    let tag = v["meta"]["version"]
        .as_str()
        .and_then(|s| HeaderValue::from_str(s).ok());
    let rid = v["id"].as_str().map(str::to_owned);
    let mut r = response(status, v);
    if let Some(tag) = tag {
        r.headers_mut().insert("etag", tag);
    }
    if let Some(rid) = rid {
        if let Ok(location) =
            HeaderValue::from_str(&format!("/management/scim/v2/{id}/{}s/{rid}", k))
        {
            r.headers_mut().insert("location", location);
        }
    }
    r
}
async fn mutate(
    state: &AppState,
    id: Uuid,
    k: &str,
    rid: Option<Uuid>,
    headers: &HeaderMap,
    body: Value,
    action: &str,
) -> Result<Response, Error> {
    let k = kind(k)?;
    let expected = if rid.is_some() {
        Some(etag(headers)?)
    } else {
        None
    };
    let mut tx = state.pool.begin().await.map_err(ApiError::from)?;
    let c = authorize(state, &mut tx, id, headers).await?;
    let hash = sha(&json!([1, id, k, rid, expected, action, body]));
    let key = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .unwrap_or(&hash)
        .to_owned();
    if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
        return Err(invalid("invalid operation key").into());
    }
    if let Some(original) = resources::replay(state, &mut tx, &c, &key, &hash).await? {
        tx.commit().await.map_err(ApiError::from)?;
        let status = StatusCode::from_u16(original["status"].as_u64().unwrap_or(200) as u16)
            .unwrap_or(StatusCode::OK);
        if status == StatusCode::NO_CONTENT {
            return Ok(status.into_response());
        }
        return Ok(resource_response(status, original["body"].clone(), id, k));
    }
    let mut profile = body.clone();
    if action == "patch" || action == "delete" {
        let row=sqlx::query("SELECT * FROM directory_resources WHERE connector_id=$1 AND id=$2 AND kind=$3 AND NOT deleted FOR UPDATE").bind(id).bind(rid).bind(k).fetch_optional(&mut *tx).await.map_err(ApiError::from)?.ok_or_else(||ApiError::NotFound("resource not found".into()))?;
        profile = resources::payload(state, &row, id).await?;
        if expected != Some(row.get("revision")) {
            return Err(conflict().into());
        }
        if action == "patch" {
            patch_profile(&mut profile, &body, k)?;
        }
    }
    let mut i = input(k, profile)?;
    if action == "delete" {
        i.deleted = true;
        i.active = false;
    }
    if action == "create" {
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM directory_resources WHERE connector_id=$1 AND kind=$2 AND external_key=$3)").bind(id).bind(k).bind(&i.external).fetch_one(&mut *tx).await.map_err(ApiError::from)?;
        if exists {
            return Err(conflict().into());
        }
    }
    let result = resources::upsert(state, &mut tx, &c, rid, expected, &i, None).await?;
    let status = if action == "create" {
        StatusCode::CREATED
    } else if action == "delete" {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::OK
    };
    let resource: Uuid = result["id"]
        .as_str()
        .unwrap()
        .parse()
        .map_err(|_| unavailable())?;
    resources::journal(state,&mut tx,&c,&key,&hash,resource,&json!({"status":status.as_u16(),"body":if status==StatusCode::NO_CONTENT{Value::Null}else{result.clone()}})).await?;
    tx.commit().await.map_err(ApiError::from)?;
    if status == StatusCode::NO_CONTENT {
        return Ok(status.into_response());
    }
    Ok(resource_response(status, result, id, k))
}
fn patch_profile(profile: &mut Value, request: &Value, k: &str) -> Result<(), ApiError> {
    let ops = request["Operations"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 100)
        .ok_or_else(|| invalid("invalid patch operations"))?;
    for op in ops {
        let action = op["op"].as_str().unwrap_or("").to_ascii_lowercase();
        if !matches!(action.as_str(), "add" | "replace" | "remove") {
            return Err(invalid("unsupported patch operation"));
        }
        let path = op["path"].as_str();
        if k == "Group"
            && path.is_some_and(|p| p.starts_with("members[value eq ") && p.ends_with(']'))
        {
            if action != "remove" {
                return Err(invalid("filtered members only supports remove"));
            }
            let raw = path
                .unwrap()
                .strip_prefix("members[value eq ")
                .unwrap()
                .strip_suffix(']')
                .unwrap();
            let target: String = serde_json::from_str(raw)?;
            let members = profile
                .get_mut("members")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| invalid("members unavailable"))?;
            members.retain(|m| m["value"] != target);
            continue;
        }
        let values = if let Some(path) = path {
            json!({(path):op["value"]})
        } else {
            op["value"].clone()
        };
        let values = values
            .as_object()
            .ok_or_else(|| invalid("patch object required"))?;
        for (field, value) in values {
            let permitted = if k == "Group" {
                matches!(field.as_str(), "displayName" | "members")
            } else {
                matches!(
                    field.as_str(),
                    "active"
                        | "userName"
                        | "displayName"
                        | "name"
                        | "emails"
                        | "phoneNumbers"
                        | "addresses"
                )
            };
            if !permitted {
                return Err(invalid("immutable or unsupported patch attribute"));
            }
            if action == "remove" {
                if matches!(field.as_str(), "userName" | "displayName") {
                    return Err(invalid("required name cannot be removed"));
                }
                profile.as_object_mut().unwrap().remove(field);
            } else if field == "members" && action == "add" {
                let new = value
                    .as_array()
                    .ok_or_else(|| invalid("members must be array"))?;
                if profile.get("members").is_none() {
                    profile["members"] = json!([]);
                }
                let existing = profile["members"]
                    .as_array_mut()
                    .ok_or_else(|| invalid("members must be array"))?;
                for m in new {
                    if !existing.iter().any(|v| v["value"] == m["value"]) {
                        existing.push(m.clone());
                    }
                }
            } else {
                profile[field] = value.clone();
            }
        }
    }
    Ok(())
}
async fn create(
    State(s): State<AppState>,
    Path((id, k)): Path<(Uuid, String)>,
    h: HeaderMap,
    b: Bytes,
) -> Result<Response, Error> {
    let v = serde_json::from_slice(&b).map_err(ApiError::from)?;
    mutate(&s, id, &k, None, &h, v, "create").await
}
async fn replace(
    State(s): State<AppState>,
    Path((id, k, r)): Path<(Uuid, String, Uuid)>,
    h: HeaderMap,
    b: Bytes,
) -> Result<Response, Error> {
    let v = serde_json::from_slice(&b).map_err(ApiError::from)?;
    mutate(&s, id, &k, Some(r), &h, v, "replace").await
}
async fn update(
    State(s): State<AppState>,
    Path((id, k, r)): Path<(Uuid, String, Uuid)>,
    h: HeaderMap,
    b: Bytes,
) -> Result<Response, Error> {
    let v = serde_json::from_slice(&b).map_err(ApiError::from)?;
    mutate(&s, id, &k, Some(r), &h, v, "patch").await
}
async fn remove(
    State(s): State<AppState>,
    Path((id, k, r)): Path<(Uuid, String, Uuid)>,
    h: HeaderMap,
) -> Result<Response, Error> {
    mutate(&s, id, &k, Some(r), &h, Value::Null, "delete").await
}
