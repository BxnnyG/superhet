//! Plugin input from the device daemon over the bus (`superhet_bus::tap`).
//!
//! For every process start the supervisor attaches one input: this module subscribes to the
//! tap's data subject, keeps the tap (and for exclusive plugins the device claim) alive with
//! a lease refreshed every few seconds, and hands the sample bytes over in a bounded queue.
//! When the plugin reads its `stdin` too slowly the queue overflows and chunks are dropped and
//! counted (`input_dropped` in the instance state) instead of stalling the bus.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_nats::Client;
use futures_util::StreamExt;
use serde_json::{Value, json};
use superhet_bus::subjects;
use superhet_bus::tap::{LEASE_REFRESH, VERSION};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::supervisor::{Attachment, InputHealth, InputRequest, Inputs};

/// Chunks buffered between the bus subscription and the plugin's `stdin` (up to ~16 MiB).
const CHUNK_QUEUE: usize = 256;
/// How long a request to the daemon may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// [`Inputs`] provider that talks to the daemons on the bus.
#[derive(Clone)]
pub struct BusInputs {
    client: Client,
}

impl BusInputs {
    /// A provider over `client`.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

impl Inputs for BusInputs {
    fn attach(&self, request: &InputRequest, health: InputHealth) -> Option<Attachment> {
        let subject = subjects::tap(&request.device, &request.tap.id).ok()?;
        let (tx, rx) = mpsc::channel(CHUNK_QUEUE);
        let receiver = tokio::spawn(receive(
            self.client.clone(),
            subject,
            tx,
            Arc::clone(&health.dropped),
        ));
        let lease = tokio::spawn(keep_alive(
            self.client.clone(),
            request.clone(),
            health.status,
        ));
        Some(Attachment {
            chunks: rx,
            guard: Box::new(Guard {
                client: self.client.clone(),
                request: request.clone(),
                tasks: [receiver, lease],
            }),
        })
    }
}

/// Releases the tap and the claim when the process it served is gone.
struct Guard {
    client: Client,
    request: InputRequest,
    tasks: [JoinHandle<()>; 2],
}

impl Drop for Guard {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let client = self.client.clone();
        let request = self.request.clone();
        // Best effort; without it the daemon expires the leases on its own.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = release(&client, &request).await;
            });
        }
    }
}

async fn receive(
    client: Client,
    subject: String,
    tx: mpsc::Sender<Vec<u8>>,
    dropped: Arc<AtomicU64>,
) {
    let mut subscription = match client.subscribe(subject).await {
        Ok(subscription) => subscription,
        Err(err) => {
            tracing::warn!(%err, "cannot subscribe to the plugin input");
            return;
        }
    };
    while let Some(message) = subscription.next().await {
        match tx.try_send(message.payload.to_vec()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                let n = dropped.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 1 || n.is_multiple_of(1000) {
                    tracing::warn!(dropped_total = n, "plugin reads its input too slowly");
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => return,
        }
    }
}

/// Send `body` to `subject` and turn a failure reply into its message.
async fn call(client: &Client, subject: String, body: Value) -> Result<(), String> {
    let payload = serde_json::to_vec(&body).map_err(|err| err.to_string())?;
    let reply = tokio::time::timeout(REQUEST_TIMEOUT, client.request(subject, payload.into()))
        .await
        .map_err(|_| "the device daemon did not answer".to_owned())?
        .map_err(|err| match err.kind() {
            async_nats::client::RequestErrorKind::NoResponders => {
                "the device daemon is not running".to_owned()
            }
            _ => format!("bus request failed: {err}"),
        })?;
    let value: Value = serde_json::from_slice(&reply.payload)
        .map_err(|_| "malformed reply from the device daemon".to_owned())?;
    if value["ok"] == true {
        Ok(())
    } else {
        Err(value["error"]
            .as_str()
            .unwrap_or("the device daemon refused the request")
            .to_owned())
    }
}

/// Claim (when needed) and open the tap; repeating it is the lease refresh.
async fn refresh(client: &Client, request: &InputRequest) -> Result<(), String> {
    let device = &request.device;
    if let Some(claim) = &request.claim {
        let subject = subjects::ctl_claim(device, "acquire").map_err(|err| err.to_string())?;
        call(client, subject, json!(claim)).await?;
    }
    let subject = subjects::ctl_tap(device, "open").map_err(|err| err.to_string())?;
    call(client, subject, json!(request.tap)).await
}

async fn release(client: &Client, request: &InputRequest) -> Result<(), String> {
    let device = &request.device;
    let by_id = json!({"v": VERSION, "id": request.tap.id});
    let subject = subjects::ctl_tap(device, "close").map_err(|err| err.to_string())?;
    let tap = call(client, subject, by_id.clone()).await;
    if request.claim.is_some() {
        let subject = subjects::ctl_claim(device, "release").map_err(|err| err.to_string())?;
        call(client, subject, by_id).await?;
    }
    tap
}

async fn keep_alive(client: Client, request: InputRequest, status: watch::Sender<Option<String>>) {
    let mut tick = tokio::time::interval(LEASE_REFRESH);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let error = refresh(&client, &request).await.err();
        if let Some(message) = &error {
            tracing::debug!(instance = %request.tap.id, %message, "plugin input not available");
        }
        status.send_if_modified(|current| {
            if *current == error {
                false
            } else {
                *current = error;
                true
            }
        });
    }
}
