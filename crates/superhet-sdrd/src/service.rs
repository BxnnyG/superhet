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
use superhet_device::DeviceControl;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use superhet_device::DeviceState;

use crate::control::{
    self, CtlError, DEBOUNCE, MAX_BATCH_WAIT, TuneJob, collect_batch, error_reply, ok_reply,
    state_json, vfo_reply,
};
use crate::vfo::{self, Tuning, VfoShared};

/// Tune requests queued between the bus loop and the executor.
const QUEUE_JOBS: usize = 64;
/// Timeout for each JetStream operation.
const JS_TIMEOUT: Duration = Duration::from_secs(5);
/// State is re-published this often, so a restarted NATS server regains it.
const REPUBLISH: Duration = Duration::from_secs(30);
/// Retry delay after a failed state publish.
const RETRY: Duration = Duration::from_secs(2);

/// Latest encoded state, `None` until the first one is known.
type StateTx = watch::Sender<Option<Arc<str>>>;

/// Running control service; abort it to stop serving.
pub struct Service {
    tasks: Vec<JoinHandle<()>>,
    client: Client,
    key: String,
}

/// Start serving control requests and publishing state for `device`.
///
/// # Errors
/// Fails if the device id cannot be used in subjects or KV keys, or the subscription is refused.
pub async fn start(
    client: Client,
    device: String,
    control: Arc<dyn DeviceControl>,
    vfos: Arc<VfoShared>,
) -> Result<Service, Box<dyn std::error::Error>> {
    let ctl_subject = subjects::ctl_all(&device)?;
    let state_subject = subjects::state(&device)?;
    let key = subjects::state_key(&device)?;

    let (state_tx, state_rx) = watch::channel(None);
    let ctx = Arc::new(Context {
        device,
        control,
        vfos,
        state: state_tx,
    });
    ctx.refresh_state(&ctx.control.state());

    let mut subscription = client.subscribe(ctl_subject).await?;
    let (job_tx, job_rx) = mpsc::channel(QUEUE_JOBS);

    let publisher = tokio::spawn(publish_state(
        client.clone(),
        state_subject,
        key.clone(),
        state_rx,
    ));
    let executor = tokio::spawn(execute_jobs(job_rx, client.clone(), Arc::clone(&ctx)));
    let server = {
        let client = client.clone();
        tokio::spawn(async move {
            while let Some(message) = subscription.next().await {
                handle_message(&client, &ctx, &job_tx, message).await;
            }
        })
    };
    Ok(Service {
        tasks: vec![publisher, executor, server],
        client,
        key,
    })
}

impl Service {
    /// Stop serving and remove the (now stale) state entry; best effort.
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

/// Everything the handlers share.
struct Context {
    device: String,
    control: Arc<dyn DeviceControl>,
    vfos: Arc<VfoShared>,
    state: StateTx,
}

impl Context {
    /// Encode the state document for `device_state` plus the current VFOs and publish it if it
    /// changed. Returns the JSON for replies.
    fn refresh_state(&self, device_state: &DeviceState) -> Arc<str> {
        let vfos = self.vfos.with(|r| r.list().to_vec());
        let json: Arc<str> = state_json(&self.device, device_state, &vfos).into();
        self.state.send_if_modified(|current| {
            if current.as_deref() == Some(&*json) {
                false
            } else {
                *current = Some(Arc::clone(&json));
                true
            }
        });
        json
    }
}

async fn respond(client: &Client, message: &Message, payload: Vec<u8>) {
    if let Some(reply) = &message.reply
        && let Err(e) = client.publish(reply.clone(), payload.into()).await
    {
        tracing::warn!(error = %e, "control reply failed");
    }
}

async fn handle_message(
    client: &Client,
    ctx: &Arc<Context>,
    jobs: &mpsc::Sender<TuneJob>,
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
                    let busy = CtlError {
                        code: control::ErrorCode::Device,
                        message: "control queue is full".into(),
                    };
                    respond(client, &message, error_reply(&busy)).await;
                }
            }
            Err(err) => respond(client, &message, error_reply(&err)).await,
        },
        "state" => {
            let current = ctx.state.borrow().clone();
            let reply = match current {
                Some(json) => ok_reply(&json),
                None => error_reply(&CtlError {
                    code: control::ErrorCode::Device,
                    message: "state not available yet".into(),
                }),
            };
            respond(client, &message, reply).await;
        }
        "vfo.create" | "vfo.update" | "vfo.delete" => {
            let reply = match tokio::task::spawn_blocking({
                let ctx = Arc::clone(ctx);
                let command = command.to_owned();
                let payload = message.payload.clone();
                move || handle_vfo(&ctx, &command, &payload)
            })
            .await
            {
                Ok(reply) => reply,
                Err(e) => {
                    tracing::error!(error = %e, "vfo task panicked");
                    error_reply(&CtlError {
                        code: control::ErrorCode::Device,
                        message: "internal error".into(),
                    })
                }
            };
            respond(client, &message, reply).await;
        }
        other => {
            tracing::debug!(device = %ctx.device, command = other, "unknown control command");
            let err = CtlError::invalid(format!("unknown command {other:?}"));
            respond(client, &message, error_reply(&err)).await;
        }
    }
}

/// Execute a VFO command and build the reply. Runs on a blocking thread because validating a
/// VFO designs its filter chain.
fn handle_vfo(ctx: &Context, command: &str, payload: &[u8]) -> Vec<u8> {
    match run_vfo(ctx, command, payload) {
        Ok(reply) => reply,
        Err(err) => {
            tracing::debug!(device = %ctx.device, command, error = %err.message, "vfo request rejected");
            error_reply(&err)
        }
    }
}

fn run_vfo(ctx: &Context, command: &str, payload: &[u8]) -> Result<Vec<u8>, CtlError> {
    let rate = ctx.control.state().sample_rate;
    let affected = match command {
        "vfo.create" => {
            let request = vfo::parse_create(payload)?;
            let vfo = ctx.vfos.with(|r| r.create(request, rate))?;
            tracing::info!(id = vfo.id, freq_hz = vfo.freq_hz, mode = ?vfo.mode, "vfo created");
            Some(vfo)
        }
        "vfo.update" => {
            let patch = vfo::parse_update(payload)?;
            let vfo = ctx.vfos.with(|r| r.update(&patch, rate))?;
            tracing::info!(id = vfo.id, freq_hz = vfo.freq_hz, mode = ?vfo.mode, "vfo updated");
            Some(vfo)
        }
        _ => {
            let id = vfo::parse_delete(payload)?;
            ctx.vfos.with(|r| r.delete(id))?;
            tracing::info!(id, "vfo deleted");
            None
        }
    };
    // Read the tuning again so a concurrent retune is not overwritten with a stale state.
    let device_state = ctx.control.state();
    let tuning = Tuning {
        center_hz: device_state.center_hz,
        sample_rate: device_state.sample_rate,
    };
    let state = ctx.refresh_state(&device_state);
    Ok(match affected {
        Some(vfo) => vfo_reply(&vfo.wire(tuning), &state),
        None => ok_reply(&state),
    })
}

async fn execute_jobs(mut jobs: mpsc::Receiver<TuneJob>, client: Client, ctx: Arc<Context>) {
    while let Some(first) = jobs.recv().await {
        let batch = collect_batch(first, &mut jobs, DEBOUNCE, MAX_BATCH_WAIT).await;
        let request = batch.request;
        let device_control = Arc::clone(&ctx.control);
        let outcome = tokio::task::spawn_blocking(move || {
            let result = device_control.apply(&request);
            // After a failure the device may be half-way applied; report what it really runs.
            let state = result
                .as_ref()
                .map_or_else(|_| device_control.state(), Clone::clone);
            (result, state)
        })
        .await;
        let (result, new_state) = match outcome {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::error!(error = %e, "control task panicked");
                continue;
            }
        };

        // The state carries the VFOs, whose status follows the new tuning.
        let json = ctx.refresh_state(&new_state);
        let reply = match result {
            Ok(_) => {
                tracing::info!(
                    merged = batch.merged,
                    center_hz = new_state.center_hz,
                    sample_rate = new_state.sample_rate,
                    epoch = new_state.tuning_epoch,
                    "tuned"
                );
                ok_reply(&json)
            }
            Err(err) => {
                tracing::warn!(error = %err, "tune failed");
                error_reply(&CtlError::from(err))
            }
        };
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
    mut latest: watch::Receiver<Option<Arc<str>>>,
) {
    let mut store: Option<kv::Store> = None;
    loop {
        let current = latest.borrow_and_update().clone();
        let delay = match current {
            Some(json) if deliver(&client, &subject, &key, &mut store, &json).await => REPUBLISH,
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
        match store {
            Some(store) => store
                .put(key, json.as_bytes().to_vec().into())
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
            None => Ok(()),
        }
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
