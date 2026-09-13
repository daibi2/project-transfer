use crate::server::config::Config;
use crate::server::errors::ServerError;
use crate::server::types::{ErrorDetail, ErrorResponse, RestoreResponse};
use crate::snapshot::{restore_home, save_home, RestoreError};
use bytes::Bytes;
use futures_core::Stream;
use http_body_util::{Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::{Request, Response, StatusCode};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

pub const TRAILER_SNAPSHOT_STATUS: &str = "x-snapshot-status";
const MAX_SAVE_REQUEST_BYTES: usize = 64 << 10;

#[derive(Default, Deserialize)]
struct SaveRequest {
    #[serde(default)]
    excludes: Vec<String>,
}

pub struct SnapshotGate {
    inner: std::sync::Mutex<()>,
}

impl SnapshotGate {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(()),
        }
    }

    pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, ()>> {
        self.inner.try_lock().ok()
    }
}

pub struct RxStream {
    rx: mpsc::Receiver<Result<Frame<Bytes>, io::Error>>,
}

impl Stream for RxStream {
    type Item = Result<Frame<Bytes>, io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

pub enum ApiBody {
    Full(Full<Bytes>),
    Stream(StreamBody<RxStream>),
}

impl hyper::body::Body for ApiBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        unsafe {
            match self.get_unchecked_mut() {
                ApiBody::Full(b) => match Pin::new(b).poll_frame(cx) {
                    Poll::Ready(Some(Ok(f))) => Poll::Ready(Some(Ok(f))),
                    Poll::Ready(Some(Err(_))) => Poll::Ready(Some(Err(io::Error::other("body")))),
                    Poll::Ready(None) => Poll::Ready(None),
                    Poll::Pending => Poll::Pending,
                },
                ApiBody::Stream(b) => Pin::new(b).poll_frame(cx),
            }
        }
    }
}

pub async fn handle_save(
    req: Request<Incoming>,
    _config: &Config,
    gate: &SnapshotGate,
) -> Response<ApiBody> {
    let collected = match http_body_util::BodyExt::collect(req.into_body()).await {
        Ok(c) => c.to_bytes(),
        Err(e) => return json_error(400, "invalid_request", e.to_string()),
    };
    let excludes = if collected.is_empty() {
        Vec::new()
    } else {
        if collected.len() > MAX_SAVE_REQUEST_BYTES {
            return json_error(400, "invalid_request", "request too large");
        }
        match serde_json::from_slice::<SaveRequest>(&collected) {
            Ok(r) => r.excludes,
            Err(e) => return json_error(400, "invalid_request", e.to_string()),
        }
    };

    let home = match resolve_home() {
        Ok(h) => h,
        Err(e) => return json_error(500, "home_unavailable", e),
    };

    let (tx, rx) = mpsc::channel::<Result<Frame<Bytes>, io::Error>>(8);
    if gate.try_lock().is_none() {
        return json_error(
            409,
            "snapshot_busy",
            "another snapshot operation is in progress",
        );
    }
    std::thread::spawn(move || {
        let mut w = ChannelWriter {
            tx: tx.clone(),
            hasher: Sha256::new(),
            n: 0,
        };
        match save_home(&mut w, &home, &excludes) {
            Ok(()) => {
                let mut trailers = hyper::HeaderMap::new();
                trailers.insert("x-snapshot-status", hyper::header::HeaderValue::from_static("ok"));
                if let Ok(v) = hyper::header::HeaderValue::from_str(&hex::encode(w.hasher.finalize()))
                {
                    trailers.insert("x-snapshot-sha256", v);
                }
                if let Ok(v) = hyper::header::HeaderValue::from_str(&w.n.to_string()) {
                    trailers.insert("x-snapshot-bytes", v);
                }
                let _ = tx.blocking_send(Ok(Frame::trailers(trailers)));
            }
            Err(e) => {
                let _ = tx.blocking_send(Err(io::Error::other(e)));
            }
        }
    });

    let body = ApiBody::Stream(StreamBody::new(RxStream { rx }));
    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "application/octet-stream")
        .header(
            hyper::header::TRAILER,
            "X-Snapshot-Status, X-Snapshot-Sha256, X-Snapshot-Bytes",
        )
        .body(body)
        .unwrap_or_else(|_| json_error(500, "snapshot_failed", "failed to build response"))
}

pub async fn handle_restore(
    req: Request<Incoming>,
    _config: &Config,
    gate: &SnapshotGate,
) -> Response<ApiBody> {
    let collected = match http_body_util::BodyExt::collect(req.into_body()).await {
        Ok(c) => c.to_bytes(),
        Err(e) => return json_error(500, "restore_failed", e.to_string()),
    };
    let _lock = match gate.try_lock() {
        Some(g) => g,
        None => {
            return json_error(
                409,
                "snapshot_busy",
                "another snapshot operation is in progress",
            )
        }
    };
    let home = match resolve_home() {
        Ok(h) => h,
        Err(e) => return json_error(500, "home_unavailable", e),
    };
    match restore_home(std::io::Cursor::new(collected), &home) {
        Ok(res) => json_ok(&RestoreResponse {
            entries: res.entries,
            bytes_written: res.bytes_written,
        }),
        Err(RestoreError::Malformed(m)) => json_error(400, "archive_malformed", m),
        Err(e) => json_error(500, "restore_failed", e.to_string()),
    }
}

fn resolve_home() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let meta = std::fs::metadata(&home).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Err("home is not a directory".into());
    }
    Ok(std::path::PathBuf::from(home))
}

struct ChannelWriter {
    tx: mpsc::Sender<Result<Frame<Bytes>, io::Error>>,
    hasher: Sha256,
    n: i64,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.n += buf.len() as i64;
        self.hasher.update(buf);
        self.tx
            .blocking_send(Ok(Frame::data(Bytes::copy_from_slice(buf))))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client gone"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn json_ok<T: serde::Serialize>(v: &T) -> Response<ApiBody> {
    json_status(200, v)
}

pub fn json_status<T: serde::Serialize>(status: u16, v: &T) -> Response<ApiBody> {
    let mut body = serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec());
    body.push(b'\n');
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(ApiBody::Full(Full::new(Bytes::from(body))))
        .unwrap()
}

pub fn json_error(status: u16, code: impl Into<String>, message: impl Into<String>) -> Response<ApiBody> {
    json_status(
        status,
        &ErrorResponse {
            error: ErrorDetail {
                code: code.into(),
                message: message.into(),
            },
        },
    )
}

pub fn write_server_error(err: ServerError) -> Response<ApiBody> {
    eprintln!("ERROR [{}] {}: {}", err.status_code, err.code, err.message);
    json_error(err.status_code, err.code, err.message)
}
