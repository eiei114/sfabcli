//! Minimal Chrome DevTools Protocol client.
//!
//! Everything here is deliberately synchronous: the CLI runs one request at a time, and blocking
//! code keeps the call sites readable.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Default CDP endpoint, matching the resident-Chrome setup these CLIs share.
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:9223";

/// A debuggable target as listed by `/json`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Target {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub url: String,
    #[serde(rename = "webSocketDebuggerUrl", default)]
    pub ws_url: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        // A 404 from /json is a diagnostic, not a transport failure.
        .http_status_as_error(false)
        .build()
        .into()
}

/// The HTTP side of CDP: listing targets and opening tabs.
pub struct Browser {
    endpoint: String,
}

impl Browser {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
        }
    }

    fn get(&self, path: &str) -> Result<String> {
        let mut response = agent()
            .get(&format!("{}{path}", self.endpoint))
            .call()
            .with_context(|| {
                format!(
                    "cannot reach CDP at {} — start Chrome with --remote-debugging-port",
                    self.endpoint
                )
            })?;
        response
            .body_mut()
            .read_to_string()
            .context("reading the CDP response")
    }

    /// Every target, including service workers and iframes.
    pub fn targets(&self) -> Result<Vec<Target>> {
        let body = self.get("/json")?;
        serde_json::from_str(&body).context("parsing /json")
    }

    /// The first page whose URL starts with `prefix`.
    pub fn find_page(&self, prefix: &str) -> Result<Option<Target>> {
        Ok(self
            .targets()?
            .into_iter()
            .find(|t| t.kind == "page" && t.url.starts_with(prefix)))
    }

    /// Open a tab. The URL goes in the query string as the endpoint expects it.
    pub fn open(&self, url: &str) -> Result<Target> {
        let mut response = agent()
            .put(&format!("{}/json/new?{url}", self.endpoint))
            .send_empty()
            .with_context(|| format!("cannot open a tab at {}", self.endpoint))?;
        let body = response
            .body_mut()
            .read_to_string()
            .context("reading /json/new")?;
        serde_json::from_str(&body).context("parsing /json/new")
    }
}

/// A websocket to one page target.
pub struct Page {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl Page {
    pub fn connect(target: &Target) -> Result<Self> {
        if target.ws_url.is_empty() {
            bail!("target {} has no websocket url", target.id);
        }
        let uri: tungstenite::http::Uri = target
            .ws_url
            .parse()
            .with_context(|| format!("bad websocket url: {}", target.ws_url))?;
        let (ws, _handshake) = tungstenite::connect(uri)
            .with_context(|| format!("connecting to {}", target.ws_url))?;
        Ok(Self { ws, next_id: 0 })
    }

    /// Send one message and return the `result` of the reply with the same id.
    ///
    /// Notifications for other ids and protocol housekeeping are skipped rather than treated as
    /// the reply; CDP may interleave both before the answer arrives.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id + 1;
        self.next_id = id;
        self.ws
            .send(Message::text(
                json!({ "id": id, "method": method, "params": params }).to_string(),
            ))
            .context("sending to CDP")?;
        loop {
            let text = match self.ws.read().context("reading from CDP")? {
                Message::Text(text) => text.as_str().to_owned(),
                Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Message::Close(frame) => bail!("CDP closed the connection: {frame:?}"),
                _ => continue,
            };
            let message: Value = serde_json::from_str(&text).context("parsing a CDP message")?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("CDP error: {}", head(&error.to_string(), 300));
            }
            let result = &message["result"];
            if let Some(details) = result.get("exceptionDetails") {
                let text = details
                    .get("exception")
                    .and_then(|e| e.get("description"))
                    .or_else(|| details.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown page error");
                bail!("page error: {}", head(text, 300));
            }
            return Ok(result
                .get("result")
                .and_then(|r| r.get("value"))
                .cloned()
                .unwrap_or(Value::Null));
        }
    }

    /// Evaluate an expression in this page and return the resolved value.
    pub fn evaluate(&mut self, expression: &str) -> Result<Value> {
        self.call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
    }

    /// Evaluate and require a string result.
    pub fn evaluate_str(&mut self, expression: &str) -> Result<String> {
        match self.evaluate(expression)? {
            Value::String(text) => Ok(text),
            other => bail!(
                "expected a string from the page, got {}",
                head(&other.to_string(), 200)
            ),
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let _ = self.ws.close(None);
    }
}

/// Truncate a message for an error, on a char boundary.
pub fn head(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}
