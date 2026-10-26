//! NATS side of device control: serves `sh.v1.dev.<dev>.ctl.*` requests and keeps the device
//! state published (core subject `state(dev)` plus KV bucket `STATE`, key `dev.<dev>`).
//!
//! VFO requests (`ctl.vfo.create|update|delete`) change the shared `VfoShared` registry, which the
//! audio engine follows, and refresh the state document (it carries the VFOs and their status).
//!
//! Tune requests are validated on arrival, folded into batches (see `control::collect_batch`)
//! and applied on a blocking thread, so slow USB control transfers never stall the bus loop.
//! State publishing is decoupled through a `watch` channel: the control path only stores the
//! newest state, a separate task delivers it and retries until the bus and JetStream are back.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::{self, kv};
use async_nats::{Client, Message, connection::State as Connection};
use futures_util::StreamExt;
use superhet_bus::subjects;
use superhet_bus::zoom::ZoomCommand;
use superhet_device::DeviceControl;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::control::{
    self, CtlError, DEBOUNCE, MAX_BATCH_WAIT, TuneJob, collect_batch, error_reply, ok_reply,
};
use crate::controller::Controller;
use crate::overload::OverloadStatus;
use crate::vfo::VfoShared;
use crate::zoom::ZoomHandle;

/// Tune requests queued between the bus loop and the executor.
const QUEUE_JOBS: usize = 64;
/// How often the ADC overload state is checked for changes.
const OVERLOAD_POLL: Duration = Duration::from_secs(1);
/// Minimum spacing of state refreshes caused by a changed dropped-block count.
const DROPPED_REFRESH: Duration = Duration::from_secs(5);
/// Timeout for each JetStream operation.
const JS_TIMEOUT: Duration = Duration::from_secs(5);
/// State is re-published this often, so a restarted NATS server regains it.
const REPUBLISH: Duration = Duration::from_secs(30);
/// Retry delay after a failed state publish.
const RETRY: Duration = Duration::from_secs(2);
/// How long start-up waits for the bus to read the saved state before it gives up restoring.
const RESTORE_WAIT: Duration = Duration::from_secs(30);
/// Pause between attempts to read the saved state.
const RESTORE_RETRY: Duration = Duration::from_millis(500);

/// Running control service; abort it to stop serving.
pub struct Service {
    tasks: Vec<JoinHandle<()>>,
    client: Client,
    key: String,
}

/// Start serving control requests and publishing state for `device`.
///
/// Before anything is published or served, the last saved state (tuning, VFOs, user lock) is
/// restored from the KV bucket, waiting up to [`RESTORE_WAIT`] for the bus. If the bus stays
/// away, the daemon starts with the device's own settings and does not overwrite the saved
/// state until it is restarted.
///
/// # Errors
/// Fails if the device id cannot be used in subjects or KV keys, or the subscription is refused.
pub async fn start(
    client: Client,
    device: String,
    control: Arc<dyn DeviceControl>,
    vfos: Arc<VfoShared>,
    zoom: ZoomHandle,
    overload: Arc<OverloadStatus>,
) -> Result<Service, Box<dyn std::error::Error>> {
    let ctl_subject = subjects::ctl_all(&device)?;
    let state_subject = subjects::state(&device)?;
    let key = subjects::state_key(&device)?;
    let restore_key = subjects::restore_key(&device)?;

    let (state_tx, state_rx) = watch::channel(None);
    let controller = Arc::new(
        Controller::new(device, control, vfos, state_tx).with_overload(Arc::clone(&overload)),
    );
    let restored = restore_saved_state(&client, &controller, &restore_key).await;
    let persist = restored.persist;
    if !restored.applied {
        // First start (or nothing usable saved): offer one VFO to listen with.
        let seeding = Arc::clone(&controller);
        if let Err(e) = tokio::task::spawn_blocking(move || seeding.seed_default_vfo()).await {
            tracing::error!(error = %e, "default vfo creation panicked");
        }
    }
    controller.refresh();

    let mut subscription = client.subscribe(ctl_subject).await?;
    let (job_tx, job_rx) = mpsc::channel(QUEUE_JOBS);

    let publisher = tokio::spawn(publish_state(
        client.clone(),
        state_subject,
        key.clone(),
        persist.then_some(restore_key),
        state_rx,
    ));
    let overload_watch = tokio::spawn(watch_overload(Arc::clone(&controller), overload));
    let executor = tokio::spawn(execute_jobs(
        job_rx,
        client.clone(),
        Arc::clone(&controller),
    ));
    let server = {
        let client = client.clone();
        tokio::spawn(async move {
            while let Some(message) = subscription.next().await {
                handle_message(&client, &controller, &job_tx, &zoom, message).await;
            }
        })
    };
    Ok(Service {
        tasks: vec![publisher, executor, server, overload_watch],
        client,
        key,
    })
}

impl Service {
    /// Stop serving and remove the (now stale) state entry; best effort. The restore snapshot
    /// stays, it is what the next start restores.
    pub async fn shutdown(self) {
        for task in &self.tasks {
            task.abort();
        }
        let delete = async {
            let js = jetstream::new(self.client.clone());
            let store = js.get_key_value(subjects::STATE_BUCKET).await.ok()?;
            store.delete(&self.key).await.ok()
        };
        if tokio::time::timeout(JS_TIMEOUT, delete)
            .await
            .ok()
            .flatten()
            .is_none()
        {
            tracing::debug!("could not remove state entry on shutdown");
        }
    }
}

/// Republish the state document when the ADC overload state changes (it is part of it).
async fn watch_overload(controller: Arc<Controller>, status: Arc<OverloadStatus>) {
    // Whole percent steps are enough; the exact clip share changes with every interval.
    let key = |status: &OverloadStatus| {
        let (percent, active) = status.get();
        (percent.round() as u32, active)
    };
    let dropped = || {
        let counters = crate::metrics::global();
        counters.device_dropped_blocks() + counters.vfo_dropped_blocks()
    };
    let mut seen = key(&status);
    let mut seen_dropped = dropped();
    let mut dropped_at = tokio::time::Instant::now();
    let mut tick = tokio::time::interval(OVERLOAD_POLL);
    loop {
        tick.tick().await;
        let now = key(&status);
        // The drop count changes with every lost block; the UI does not need more than a
        // fresh value every few seconds, so a sustained overrun cannot flood the state feed.
        let now_dropped = dropped();
        let dropped_changed =
            now_dropped != seen_dropped && dropped_at.elapsed() >= DROPPED_REFRESH;
        if now != seen || dropped_changed {
            seen = now;
            if dropped_changed {
                seen_dropped = now_dropped;
                dropped_at = tokio::time::Instant::now();
            }
            let controller = Arc::clone(&controller);
            if tokio::task::spawn_blocking(move || controller.refresh())
                .await
                .is_err()
            {
                tracing::error!("state refresh after an overload change panicked");
            }
        }
    }
}

/// Outcome of reading the restore snapshot.
struct Restored {
    /// The outcome is known (found, incompatible or nothing saved), i.e. it is safe to start
    /// overwriting the snapshot.
    persist: bool,
    /// A snapshot was applied; its VFO set (even an empty one) is the user's.
    applied: bool,
}

/// Read the restore snapshot and apply it.
async fn restore_saved_state(
    client: &Client,
    controller: &Arc<Controller>,
    restore_key: &str,
) -> Restored {
    let deadline = tokio::time::Instant::now() + RESTORE_WAIT;
    let saved = loop {
        if client.connection_state() == Connection::Connected {
            let read = async {
                let store = ensure_bucket(client).await?;
                store.get(restore_key).await.map_err(|e| e.to_string())
            };
            match tokio::time::timeout(JS_TIMEOUT, read).await {
                Ok(Ok(saved)) => break saved,
                Ok(Err(e)) => tracing::debug!(error = %e, "reading the saved state failed"),
                Err(_) => tracing::debug!("reading the saved state timed out"),
            }
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                "bus not usable within {}s; starting without restoring the saved state \
                 (it will not be overwritten until the next start)",
                RESTORE_WAIT.as_secs()
            );
            return Restored {
                persist: false,
                applied: false,
            };
        }
        tokio::time::sleep(RESTORE_RETRY).await;
    };
    let Some(saved) = saved else {
        tracing::info!("no saved state to restore");
        return Restored {
            persist: true,
            applied: false,
        };
    };
    let controller = Arc::clone(controller);
    let outcome = tokio::task::spawn_blocking(move || {
        let doc: serde_json::Value =
            serde_json::from_slice(&saved).map_err(|e| format!("unreadable: {e}"))?;
        controller.restore(&doc)
    })
    .await;
    let applied = match outcome {
        Ok(Ok(())) => true,
        Ok(Err(reason)) => {
            tracing::warn!(%reason, "saved state not restored");
            false
        }
        Err(e) => {
            tracing::error!(error = %e, "state restore panicked");
            false
        }
    };
    Restored {
        persist: true,
        applied,
    }
}

async fn respond(client: &Client, message: &Message, payload: Vec<u8>) {
    if let Some(reply) = &message.reply
        && let Err(e) = client.publish(reply.clone(), payload.into()).await
    {
        tracing::warn!(error = %e, "control reply failed");
    }
}

/// Run a blocking controller operation and turn its result into a reply.
async fn run_blocking<F>(f: F) -> Vec<u8>
where
    F: FnOnce() -> Result<Vec<u8>, CtlError> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(err)) => {
            tracing::debug!(error = %err.message, "control request rejected");
            error_reply(&err)
        }
        Err(e) => {
            tracing::error!(error = %e, "control task panicked");
            Controller::internal_error()
        }
    }
}

async fn handle_message(
    client: &Client,
    controller: &Arc<Controller>,
    jobs: &mpsc::Sender<TuneJob>,
    zoom: &ZoomHandle,
    message: Message,
) {
    let command = message
        .subject
        .as_str()
        .split_once(".ctl.")
        .map_or("", |(_, command)| command);
    match command {
        "tune" => match control::parse_tune(&message.payload) {
            Ok(request) => {
                let job = TuneJob {
                    request,
                    reply: message.reply.clone(),
                };
                if jobs.try_send(job).is_err() {
                    let busy = CtlError::new(control::ErrorCode::Device, "control queue is full");
                    respond(client, &message, error_reply(&busy)).await;
                }
            }
            Err(err) => respond(client, &message, error_reply(&err)).await,
        },
        // Fire-and-forget lease of the zoom spectrum slice; a reply is only sent on errors.
        "spectrum.zoom" => match ZoomCommand::decode(&message.payload) {
            Ok(command) => zoom.set(command),
            Err(err) => {
                tracing::debug!(%err, "zoom request rejected");
                let err = CtlError::invalid(err.to_string());
                respond(client, &message, error_reply(&err)).await;
            }
        },
        "state" => {
            let reply = match controller.latest() {
                Some(json) => ok_reply(&json),
                None => error_reply(&CtlError::new(
                    control::ErrorCode::Device,
                    "state not available yet",
                )),
            };
            respond(client, &message, reply).await;
        }
        "vfo.create" | "vfo.update" | "vfo.delete" => {
            // Validating a VFO designs its filter chain, hence the blocking thread.
            let reply = run_blocking({
                let controller = Arc::clone(controller);
                let command = command.to_owned();
                let payload = message.payload.clone();
                move || controller.vfo(&command, &payload)
            })
            .await;
            respond(client, &message, reply).await;
        }
        "lock.acquire" | "lock.release" => {
            let acquire = command == "lock.acquire";
            let reply = run_blocking({
                let controller = Arc::clone(controller);
                let payload = message.payload.clone();
                move || {
                    control::parse_empty(&payload)?;
                    Ok(ok_reply(&controller.set_user_lock(acquire)))
                }
            })
            .await;
            respond(client, &message, reply).await;
        }
        "profile.load" => {
            let reply = run_blocking({
                let controller = Arc::clone(controller);
                let payload = message.payload.clone();
                move || {
                    controller
                        .load_profile(&payload)
                        .map(|json| ok_reply(&json))
                }
            })
            .await;
            respond(client, &message, reply).await;
        }
        other => {
            tracing::debug!(command = other, "unknown control command");
            let err = CtlError::invalid(format!("unknown command {other:?}"));
            respond(client, &message, error_reply(&err)).await;
        }
    }
}

async fn execute_jobs(
    mut jobs: mpsc::Receiver<TuneJob>,
    client: Client,
    controller: Arc<Controller>,
) {
    while let Some(first) = jobs.recv().await {
        let batch = collect_batch(first, &mut jobs, DEBOUNCE, MAX_BATCH_WAIT).await;
        let request = batch.request;
        let reply = run_blocking({
            let controller = Arc::clone(&controller);
            move || controller.tune(&request).map(|json| ok_reply(&json))
        })
        .await;
        tracing::debug!(merged = batch.merged, "tune batch executed");
        for subject in batch.replies {
            if let Err(e) = client.publish(subject, reply.clone().into()).await {
                tracing::warn!(error = %e, "control reply failed");
            }
        }
    }
}

/// Deliver every new state to the state subject and the KV bucket.
async fn publish_state(
    client: Client,
    subject: String,
    key: String,
    restore_key: Option<String>,
    mut latest: watch::Receiver<Option<Arc<str>>>,
) {
    let mut store: Option<kv::Store> = None;
    loop {
        let current = latest.borrow_and_update().clone();
        let delay = match current {
            Some(json)
                if deliver(
                    &client,
                    &subject,
                    &key,
                    restore_key.as_deref(),
                    &mut store,
                    &json,
                )
                .await =>
            {
                REPUBLISH
            }
            Some(_) => RETRY,
            None => REPUBLISH,
        };
        tokio::select! {
            changed = latest.changed() => if changed.is_err() { return },
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// One delivery attempt; `true` when both the subject and KV were served.
async fn deliver(
    client: &Client,
    subject: &str,
    key: &str,
    restore_key: Option<&str>,
    store: &mut Option<kv::Store>,
    json: &Arc<str>,
) -> bool {
    if client.connection_state() != Connection::Connected {
        return false;
    }
    if let Err(e) = client
        .publish(subject.to_owned(), json.as_bytes().to_vec().into())
        .await
    {
        tracing::warn!(error = %e, "state publish failed");
        return false;
    }
    let put = async {
        if store.is_none() {
            *store = Some(ensure_bucket(client).await?);
        }
        let Some(store) = store else {
            return Ok::<(), String>(());
        };
        for key in std::iter::once(key).chain(restore_key) {
            store
                .put(key, json.as_bytes().to_vec().into())
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    match tokio::time::timeout(JS_TIMEOUT, put).await {
        Ok(Ok(())) => true,
        Ok(Err(e)) => report_kv_failure(&e, store),
        Err(_) => report_kv_failure(&"timed out", store),
    }
}

fn report_kv_failure(error: &dyn std::fmt::Display, store: &mut Option<kv::Store>) -> bool {
    tracing::warn!(%error, "state KV update failed; will retry");
    *store = None;
    false
}

async fn ensure_bucket(client: &Client) -> Result<kv::Store, String> {
    let js = jetstream::new(client.clone());
    if let Ok(store) = js.get_key_value(subjects::STATE_BUCKET).await {
        return Ok(store);
    }
    tracing::info!(bucket = subjects::STATE_BUCKET, "creating state KV bucket");
    js.create_key_value(kv::Config {
        bucket: subjects::STATE_BUCKET.to_owned(),
        description: "Latest state of every device".to_owned(),
        history: 1,
        ..kv::Config::default()
    })
    .await
    .map_err(|e| e.to_string())
}
