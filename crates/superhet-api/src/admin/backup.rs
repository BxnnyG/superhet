//! Backup and restore (`/api/v1/admin/backup`, docs/backup.md, ADR-0030). Admin only.
//!
//! The archive upload is untrusted input: it is streamed into an anonymous temporary file under a
//! hard size cap, decrypted (if sealed) into another, and only then opened by
//! `superhet-backup`, which checks entries, sizes and checksums before the database is touched.

use std::io::{Read, Seek};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Extension,
    body::{Body, Bytes},
    extract::{Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use superhet_backup::{
    Error as BackupError, ExportOptions, ImportOptions, archive::Limits, crypto, schedule,
};
use superhet_bus::audit::Outcome;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

use super::{AdminState, bad, conflict};
use crate::access::{Actor, Principal};
use crate::auth::{Reply, error, flatten, guard_origin, json_reply, parse_json};

/// One backup operation at a time: they are heavy and the import replaces data.
static BUSY: Semaphore = Semaphore::const_new(1);

/// Default upload cap in MiB (`SUPERHET_BACKUP_MAX_UPLOAD_MB`).
const DEFAULT_MAX_UPLOAD_MB: u64 = 2048;
/// Name of the settings document the worker keeps for the schedule status.
const STATUS_KEY: &str = "backup_status";

fn max_upload_bytes() -> u64 {
    std::env::var("SUPERHET_BACKUP_MAX_UPLOAD_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|mb| *mb > 0)
        .unwrap_or(DEFAULT_MAX_UPLOAD_MB)
        .saturating_mul(1 << 20)
}

fn recordings_dir() -> PathBuf {
    std::env::var_os("SUPERHET_RECORDINGS_DIR")
        .filter(|d| !d.is_empty())
        .map_or_else(|| PathBuf::from("/data/recordings"), PathBuf::from)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Map a backup error to the HTTP answer.
fn failed(err: BackupError) -> Response {
    let (status, code) = match &err {
        BackupError::BadArchive(_) => (StatusCode::BAD_REQUEST, "bad_backup"),
        BackupError::Newer(_) => (StatusCode::CONFLICT, "backup_newer"),
        BackupError::PassphraseRequired => (StatusCode::BAD_REQUEST, "passphrase_required"),
        BackupError::WrongPassphrase => (StatusCode::BAD_REQUEST, "wrong_passphrase"),
        BackupError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid"),
        BackupError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
        BackupError::Db(_) => (StatusCode::UNPROCESSABLE_ENTITY, "database_refused"),
        BackupError::Io(_) => {
            tracing::warn!(%err, "backup i/o failed");
            return crate::auth::internal();
        }
    };
    if matches!(err, BackupError::Db(_)) {
        tracing::warn!(%err, "backup database error");
    }
    error(status, code, Some(&err.to_string()))
}

fn user_id(me: &Principal) -> Result<i64, Response> {
    match me.actor {
        Actor::User(id) => Ok(id),
        Actor::Guest(_) => Err(bad("only accounts can run backups")),
    }
}

fn busy() -> Response {
    conflict("busy", "another backup operation is running")
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `GET /api/v1/admin/backup`: what can be backed up and how the schedule is doing.
pub(super) async fn overview(State(state): State<AdminState>) -> Response {
    flatten(overview_inner(&state).await)
}

async fn overview_inner(state: &AdminState) -> Reply {
    let db = state.auth.db()?;
    let status = superhet_db::settings::get(db, STATUS_KEY)
        .await
        .map_err(crate::auth::db_failed)?;
    let areas: Vec<Value> = superhet_backup::AREAS
        .iter()
        .map(|a| {
            json!({
                "id": a.id, "title": a.title, "description": a.description,
                "optional": a.optional, "credentials": a.credentials,
            })
        })
        .collect();
    Ok(json_reply(
        StatusCode::OK,
        &json!({
            "v": 1,
            "areas": areas,
            "max_upload_bytes": max_upload_bytes(),
            "min_passphrase_length": crypto::PASSPHRASE_MIN,
            "schedule": status,
        }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportBody {
    #[serde(default)]
    areas: Vec<String>,
    #[serde(default)]
    include_credentials: bool,
    #[serde(default)]
    passphrase: Option<String>,
}

/// `POST /api/v1/admin/backup/export`: build an archive and stream it back.
pub(super) async fn export(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(export_inner(&state, &me, &headers, &body).await)
}

async fn export_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let db = state.auth.db()?;
    let req: ExportBody = parse_json(headers, body)?;
    user_id(me)?;
    let passphrase = req.passphrase.filter(|p| !p.is_empty());
    if let Some(p) = &passphrase {
        crypto::check_passphrase(p).map_err(failed)?;
    }
    let _permit = BUSY.try_acquire().map_err(|_| busy())?;
    let options = ExportOptions {
        areas: req.areas,
        include_credentials: req.include_credentials,
        created_by: me.name.clone(),
        recordings_dir: Some(recordings_dir()),
    };
    let result = async {
        let (mut file, manifest) = superhet_backup::export(db, &options).await?;
        if let Some(pass) = passphrase.clone() {
            file = tokio::task::spawn_blocking(move || {
                let mut sealed = superhet_backup::temp_file()?;
                crypto::encrypt(&mut file, &mut sealed, &pass)?;
                sealed.rewind()?;
                Ok::<_, BackupError>(sealed)
            })
            .await
            .map_err(|e| BackupError::Io(std::io::Error::other(e.to_string())))??;
        }
        Ok::<_, BackupError>((file, manifest))
    }
    .await;
    let (file, manifest) = match result {
        Ok(ok) => ok,
        Err(err) => {
            state
                .auth
                .audit()
                .log(
                    &me.who(),
                    "backup.export",
                    None,
                    Outcome::Error,
                    json!({"error": err.to_string()}),
                )
                .await;
            return Err(failed(err));
        }
    };
    let bytes = file.metadata().map_err(|e| failed(e.into()))?.len();
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "backup.export",
            None,
            Outcome::Ok,
            json!({
                "areas": manifest.areas,
                "credentials": manifest.includes_credentials,
                "encrypted": passphrase.is_some(),
                "bytes": bytes,
            }),
        )
        .await;
    let name = schedule::file_name(now_secs(), passphrase.is_some());
    let stream =
        futures_util::stream::unfold(Some(tokio::fs::File::from_std(file)), |state| async move {
            let mut f = state?;
            let mut buf = vec![0u8; 256 * 1024];
            match f.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok::<_, std::io::Error>(Bytes::from(buf)), Some(f)))
                }
                Err(e) => Some((Err(e), None)),
            }
        });
    let mut response = Response::new(Body::from_stream(stream));
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(bytes));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportQuery {
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    overwrite_self: bool,
    /// Comma-separated area ids; empty means everything in the archive.
    #[serde(default)]
    areas: String,
}

/// `POST /api/v1/admin/backup/import`: dry run or apply an uploaded archive (raw request body).
pub(super) async fn import(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Query(query): Query<ImportQuery>,
    request: Request,
) -> Response {
    flatten(import_inner(&state, &me, query, request).await)
}

/// Stream the request body into an anonymous temp file, refusing more than `cap` bytes.
async fn receive(body: Body, declared: Option<u64>, cap: u64) -> Result<std::fs::File, Response> {
    if declared.is_some_and(|n| n > cap) {
        return Err(too_large(cap));
    }
    let file = superhet_backup::temp_file().map_err(|e| failed(e.into()))?;
    let mut out = tokio::fs::File::from_std(file);
    let mut stream = body.into_data_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| bad("the upload was interrupted"))?;
        total += chunk.len() as u64;
        if total > cap {
            return Err(too_large(cap));
        }
        out.write_all(&chunk).await.map_err(|e| failed(e.into()))?;
    }
    out.flush().await.map_err(|e| failed(e.into()))?;
    let mut file = out.into_std().await;
    file.rewind().map_err(|e| failed(e.into()))?;
    Ok(file)
}

fn too_large(cap: u64) -> Response {
    error(
        StatusCode::PAYLOAD_TOO_LARGE,
        "too_large",
        Some(&format!(
            "the backup is larger than {} MiB (SUPERHET_BACKUP_MAX_UPLOAD_MB)",
            cap >> 20
        )),
    )
}

/// Decrypt if sealed, then open and verify. Blocking: run off the async threads.
fn prepare(
    mut file: std::fs::File,
    passphrase: Option<String>,
    cap: u64,
) -> Result<superhet_backup::Opened, BackupError> {
    let mut head = [0u8; 8];
    let n = file.read(&mut head)?;
    file.rewind()?;
    if crypto::is_encrypted(&head[..n]) {
        let pass = passphrase.ok_or(BackupError::PassphraseRequired)?;
        let mut plain = superhet_backup::temp_file()?;
        crypto::decrypt(&mut file, &mut plain, &pass, cap)?;
        plain.rewind()?;
        file = plain;
    }
    let limits = Limits::default();
    superhet_backup::archive::open(file, limits)
}

async fn import_inner(
    state: &AdminState,
    me: &Principal,
    query: ImportQuery,
    request: Request,
) -> Reply {
    guard_origin(&state.auth, request.headers())?;
    let db = state.auth.db()?;
    let me_id = user_id(me)?;
    let passphrase = match request.headers().get("x-backup-passphrase") {
        None => None,
        Some(v) => Some(
            v.to_str()
                .ok()
                .and_then(percent_decode)
                .ok_or_else(|| bad("invalid passphrase header"))?,
        ),
    }
    .filter(|p| !p.is_empty());
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let areas: Vec<String> = query
        .areas
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .collect();
    let _permit = BUSY.try_acquire().map_err(|_| busy())?;
    let cap = max_upload_bytes();
    let file = receive(request.into_body(), declared, cap).await?;
    let mut opened = tokio::task::spawn_blocking(move || prepare(file, passphrase, cap))
        .await
        .map_err(|_| crate::auth::internal())?
        .map_err(failed)?;
    let options = ImportOptions {
        areas,
        overwrite_self: query.overwrite_self,
        self_user_id: Some(me_id),
        dry_run: query.dry_run,
        recordings_dir: Some(recordings_dir()),
    };
    let action = if query.dry_run {
        "backup.import_dry_run"
    } else {
        "backup.import"
    };
    match superhet_backup::import(db, &mut opened, &options).await {
        Ok(report) => {
            state
                .auth
                .audit()
                .log(
                    &me.who(),
                    action,
                    None,
                    Outcome::Ok,
                    json!({
                        "areas": report.areas,
                        "backup_created_at": opened.manifest.created_at,
                        "overwrite_self": query.overwrite_self,
                        "rows_inserted": report.tables.iter().map(|t| t.inserted).sum::<u64>(),
                        "files_restored": report.files_restored,
                    }),
                )
                .await;
            Ok(json_reply(
                StatusCode::OK,
                &json!({"v": 1, "report": report}),
            ))
        }
        Err(err) => {
            state
                .auth
                .audit()
                .log(
                    &me.who(),
                    action,
                    None,
                    Outcome::Error,
                    json!({"error": err.to_string()}),
                )
                .await;
            Err(failed(err))
        }
    }
}
