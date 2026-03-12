//! Webhook delivery for task and chain completion events.
//!
//! Webhooks are registered via `POST /v1/webhooks` and fire HTTP POST requests
//! when matching events occur. Delivery is best-effort (fire-and-forget).
//!
//! Currently supports HTTP URLs only. HTTPS support requires adding a TLS
//! connector (e.g. `hyper-rustls` or `reqwest`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Events that can trigger a webhook.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEvent {
    TaskCompleted,
    TaskFailed,
    ChainCompleted,
    ChainFailed,
    DrainCompleted,
}

/// A registered webhook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    /// Unique ID assigned on registration.
    pub id: String,
    /// URL to POST to (HTTP only for now).
    pub url: String,
    /// Events that trigger this webhook. Empty means all events.
    pub events: Vec<WebhookEvent>,
}

/// Payload sent to webhook endpoints.
#[derive(Debug, Serialize)]
pub struct WebhookPayload {
    pub event: WebhookEvent,
    pub timestamp_ms: u128,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

/// Thread-safe webhook registry.
#[derive(Debug, Clone, Default)]
pub struct WebhookRegistry {
    hooks: Arc<RwLock<Vec<Webhook>>>,
}

impl WebhookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new webhook, returning its assigned ID.
    pub async fn register(&self, url: String, events: Vec<WebhookEvent>) -> String {
        let id = generate_id();
        let hook = Webhook {
            id: id.clone(),
            url,
            events,
        };
        self.hooks.write().await.push(hook);
        id
    }

    /// List all registered webhooks.
    pub async fn list(&self) -> Vec<Webhook> {
        self.hooks.read().await.clone()
    }

    /// Remove a webhook by ID. Returns true if found.
    pub async fn remove(&self, id: &str) -> bool {
        let mut hooks = self.hooks.write().await;
        let len = hooks.len();
        hooks.retain(|h| h.id != id);
        hooks.len() < len
    }

    /// Fire all matching webhooks for a given event.
    ///
    /// Delivery is best-effort: failures are logged but do not block.
    pub async fn fire(&self, event: WebhookEvent, data: serde_json::Value) {
        let hooks = self.hooks.read().await;
        let matching: Vec<Webhook> = hooks
            .iter()
            .filter(|h| h.events.is_empty() || h.events.contains(&event))
            .cloned()
            .collect();
        drop(hooks);

        if matching.is_empty() {
            return;
        }

        let payload = WebhookPayload {
            event,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            data,
        };

        let body = match serde_json::to_vec(&payload) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("failed to serialize webhook payload: {e}");
                return;
            }
        };

        for hook in matching {
            let body = body.clone();
            tokio::spawn(async move {
                deliver(&hook, &body).await;
            });
        }
    }
}

/// Deliver a webhook payload via a raw HTTP/1.1 POST over TCP.
///
/// This is a minimal implementation that avoids additional dependencies.
/// For production use with HTTPS, add `reqwest` or `hyper-rustls`.
async fn deliver(hook: &Webhook, body: &[u8]) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let url = match url_parts(&hook.url) {
        Some(parts) => parts,
        None => {
            tracing::warn!(webhook_id = %hook.id, "invalid webhook URL: {}", hook.url);
            return;
        }
    };

    let addr = format!("{}:{}", url.host, url.port);
    let mut stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(webhook_id = %hook.id, "connection failed: {e}");
            return;
        }
    };

    let request = format!(
        "POST {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         User-Agent: claude-pool-server\r\n\
         Connection: close\r\n\
         \r\n",
        url.path,
        url.host,
        body.len(),
    );

    if let Err(e) = stream.write_all(request.as_bytes()).await {
        tracing::warn!(webhook_id = %hook.id, "write headers failed: {e}");
        return;
    }
    if let Err(e) = stream.write_all(body).await {
        tracing::warn!(webhook_id = %hook.id, "write body failed: {e}");
        return;
    }

    // Read just the status line to log the result.
    let mut buf = [0u8; 128];
    match stream.read(&mut buf).await {
        Ok(n) if n > 0 => {
            let response = String::from_utf8_lossy(&buf[..n]);
            let status = response.lines().next().unwrap_or("unknown");
            tracing::debug!(webhook_id = %hook.id, status, "webhook delivered");
        }
        Ok(_) => {
            tracing::debug!(webhook_id = %hook.id, "webhook delivered (empty response)");
        }
        Err(e) => {
            tracing::warn!(webhook_id = %hook.id, "read response failed: {e}");
        }
    }
}

struct UrlParts {
    host: String,
    port: u16,
    path: String,
}

fn url_parts(url: &str) -> Option<UrlParts> {
    let rest = url.strip_prefix("http://")?;
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match host_port.find(':') {
        Some(i) => (&host_port[..i], host_port[i + 1..].parse().ok()?),
        None => (host_port, 80),
    };
    Some(UrlParts {
        host: host.to_string(),
        port,
        path: path.to_string(),
    })
}

fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let random = ts ^ (std::process::id() as u128 * 6_364_136_223_846_793_005);
    format!("wh_{random:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_basic() {
        let parts = url_parts("http://localhost:8080/hooks/test").unwrap();
        assert_eq!(parts.host, "localhost");
        assert_eq!(parts.port, 8080);
        assert_eq!(parts.path, "/hooks/test");
    }

    #[test]
    fn parse_url_default_port() {
        let parts = url_parts("http://example.com/webhook").unwrap();
        assert_eq!(parts.host, "example.com");
        assert_eq!(parts.port, 80);
        assert_eq!(parts.path, "/webhook");
    }

    #[test]
    fn parse_url_no_path() {
        let parts = url_parts("http://localhost:3000").unwrap();
        assert_eq!(parts.host, "localhost");
        assert_eq!(parts.port, 3000);
        assert_eq!(parts.path, "/");
    }

    #[test]
    fn parse_url_https_rejected() {
        assert!(url_parts("https://example.com/hook").is_none());
    }

    #[tokio::test]
    async fn registry_crud() {
        let reg = WebhookRegistry::new();
        assert!(reg.list().await.is_empty());

        let id = reg
            .register(
                "http://localhost:9999/hook".into(),
                vec![WebhookEvent::TaskCompleted],
            )
            .await;
        assert!(id.starts_with("wh_"));
        assert_eq!(reg.list().await.len(), 1);

        assert!(reg.remove(&id).await);
        assert!(reg.list().await.is_empty());

        // Remove non-existent returns false.
        assert!(!reg.remove("wh_nonexistent").await);
    }
}
