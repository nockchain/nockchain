use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use axum::routing::get;
use axum::{serve, Router};
use axum_server::tls_rustls::RustlsConfig;
use nockvm::noun::{Atom, D, T};
use nockvm_macros::tas;
use tokio::select;
use tokio::sync::{oneshot, RwLock};
use tower_http::services::ServeDir;
use tracing::{debug, error, info, warn};

use crate::drivers::http::acme::AcmeManager;
use crate::nockapp::driver::{make_driver, IODriverFn, PokeResult};
use crate::nockapp::wire::{Wire, WireRepr};
use crate::nockapp::NockAppError;
use crate::noun::slab::NounSlab;
use crate::{AtomExt, Bytes};

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("Failed to bind TCP listener: {0}")]
    BindError(#[from] std::io::Error),
    #[error("Failed to get local address")]
    LocalAddrError,
    #[error("Failed to serve HTTP: {0}")]
    ServeError(String),
    #[error("Channel closed unexpectedly")]
    ChannelClosed,
    #[error("Failed to create atom from value: {0}")]
    AtomCreationError(String),
    #[error("Invalid header name")]
    InvalidHeaderName,
    #[error("Invalid header value: {0}")]
    InvalidHeaderValue(#[from] axum::http::header::ToStrError),
    #[error("Body length conversion failed")]
    BodyLengthConversion,
    #[error("Effect processing failed: {0}")]
    EffectError(#[from] NockAppError),
    #[error("Invalid UTF-8 in response: {0}")]
    Utf8Error(#[from] std::string::FromUtf8Error),
    #[error("Invalid response body")]
    InvalidResponseBody,
    #[error("Response channel not found for id: {0}")]
    ResponseChannelNotFound(u64),
    #[error("Failed to build HTTP response: {0}")]
    ResponseBuildError(#[from] axum::http::Error),
    #[error("ACME error: {0}")]
    AcmeError(#[from] anyhow::Error),
    #[error("Environment variable error: {0}")]
    EnvError(#[from] env::VarError),
    #[error("Noun processing error: {0}")]
    NounError(#[from] nockvm::noun::Error),
}

impl From<HttpError> for NockAppError {
    fn from(err: HttpError) -> Self {
        match err {
            HttpError::BindError(io_err) => NockAppError::IoError(io_err),
            HttpError::EffectError(nock_err) => nock_err,
            HttpError::Utf8Error(utf8_err) => NockAppError::FromUtf8Error(utf8_err),
            _ => NockAppError::OtherError(String::from("HttpError: Unknown error")),
        }
    }
}

type Responder = oneshot::Sender<Result<Response, StatusCode>>;
#[derive(Debug)]
struct RequestMessage {
    id: u64,
    uri: Uri,
    method: Method,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
    resp: Responder,
}

struct ResponseBuilder {
    status_code: StatusCode,
    headers: Vec<(String, String)>,
    body: Option<axum::body::Bytes>,
}

pub enum HttpWire {
    Request,
}

impl Wire for HttpWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "http";

    fn to_wire(&self) -> WireRepr {
        let tags = match self {
            HttpWire::Request => vec!["req".into()],
        };
        WireRepr::new(HttpWire::SOURCE, HttpWire::VERSION, tags)
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);
// wraps on overflow
fn get_id() -> u64 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
struct CachedResponse {
    status: StatusCode,
    headers: Vec<(String, String)>,
    body: Option<Bytes>,
    timestamp: Instant,
}

impl CachedResponse {
    fn new(status: StatusCode, headers: Vec<(String, String)>, body: Option<Bytes>) -> Self {
        Self {
            status,
            headers,
            body,
            timestamp: Instant::now(),
        }
    }

    fn is_expired(&self, max_age: Duration) -> bool {
        self.timestamp.elapsed() > max_age
    }

    fn to_response(&self) -> Result<Response<Body>, HttpError> {
        let mut res = Response::builder().status(self.status);
        for (k, v) in &self.headers {
            res = res.header(k, v);
        }
        let body = self
            .body
            .as_ref()
            .map(|b| Body::from(b.clone()))
            .unwrap_or_else(Body::empty);
        res.body(body).map_err(HttpError::ResponseBuildError)
    }
}

#[derive(Clone)]
struct AppState {
    sender: Arc<RwLock<tokio::sync::mpsc::Sender<RequestMessage>>>,
    challenges: Option<Arc<RwLock<HashMap<String, String>>>>,
}

/// ACME challenge handler for Let's Encrypt HTTP-01 validation
async fn acme_challenge_handler(
    Path(token): Path<String>,
    State(state): State<AppState>,
) -> Result<String, StatusCode> {
    if let Some(challenges) = &state.challenges {
        let challenges_guard = challenges.read().await;
        if let Some(key_authorization) = challenges_guard.get(&token) {
            debug!("Serving ACME challenge for token: {}", token);
            return Ok(key_authorization.clone());
        }
    }
    debug!("ACME challenge token not found: {}", token);
    Err(StatusCode::NOT_FOUND)
}

/// HTTP IO driver with support for automatic HTTPS via Let's Encrypt
pub fn http() -> IODriverFn {
    make_driver(move |handle| async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<RequestMessage>(10);

        // Domain to bind to for HTTPS
        let domain = env::var("HTTPS_DOMAIN").unwrap_or_else(|_| "localhost".to_string());
        // Directory to serve static files from
        let web_dir = env::var("WEB_DIR").ok();

        // Check if we're running locally
        let is_local = domain == "localhost"
            || domain.starts_with("127.")
            || domain.starts_with("192.168.")
            || domain.ends_with(".local");

        info!(
            "HTTP Driver starting - Domain: {}, Local mode: {}",
            domain, is_local
        );

        let (app_state, acme_manager_opt) = if is_local {
            debug!("Running in local mode on domain: {}", domain);
            (
                AppState {
                    sender: Arc::new(RwLock::new(tx.clone())),
                    challenges: None,
                },
                None,
            )
        } else {
            // Email to use for ACME account
            let email = env::var("ACME_EMAIL").map_err(HttpError::EnvError)?;
            // Directory to store ACME challenge responses
            let cache_dir = env::var("ACME_CACHE_DIR")
                .map(|s| s.into())
                .unwrap_or_else(|_| crate::system_data_dir().join("acme"));
            info!("HTTPS enabled with domain: {}, email: {}", domain, email);
            info!("Setting up Let's Encrypt for domain: {}", domain);
            let acme_manager = AcmeManager::new(domain.clone(), email.clone(), cache_dir.clone())
                .await
                .map_err(HttpError::AcmeError)?;

            let challenges = acme_manager.get_challenge_handler();

            (
                AppState {
                    sender: Arc::new(RwLock::new(tx.clone())),
                    challenges: Some(challenges),
                },
                Some(acme_manager),
            )
        };

        let app = if is_local {
            // For local development, just use the main handler + static file serving
            let mut router = Router::new().route("/favicon.ico", get(favicon_handler));

            if let Some(web_dir_path) = &web_dir {
                info!(
                    "Static file serving enabled from directory: {} at /static/*",
                    web_dir_path
                );
                let serve_dir = ServeDir::new(web_dir_path);
                router = router
                    .nest_service("/static", serve_dir)
                    .fallback(nockvm_handler);
            } else {
                router = router.fallback(nockvm_handler);
            }
            router.with_state(app_state.clone())
        } else {
            // For production, include ACME challenge handler
            let mut router = Router::new()
                .route("/favicon.ico", get(favicon_handler))
                .route(
                    "/.well-known/acme-challenge/{token}",
                    get(acme_challenge_handler),
                );

            if let Some(web_dir_path) = &web_dir {
                info!(
                    "Static file serving enabled from directory: {} at /static/*",
                    web_dir_path
                );
                let serve_dir = ServeDir::new(web_dir_path);
                router = router
                    .nest_service("/static", serve_dir)
                    .fallback(nockvm_handler);
            } else {
                router = router.fallback(nockvm_handler);
            }
            router.with_state(app_state.clone())
        };

        if is_local {
            // Local development: just run HTTP on port 8080
            let http_listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
                .await
                .map_err(HttpError::BindError)?;
            let http_addr = http_listener
                .local_addr()
                .map_err(|_| HttpError::LocalAddrError)?;
            info!("Local HTTP server listening on http://{}", http_addr);

            tokio::spawn(async move {
                if let Err(e) = serve(http_listener, app.into_make_service()).await {
                    error!("HTTP server error: {}", e);
                }
            });
        } else {
            // Production: Start HTTP server first for ACME challenges
            info!("Starting HTTP server for ACME challenges");
            let http_app = app.clone();
            let http_listener = tokio::net::TcpListener::bind("0.0.0.0:80")
                .await
                .map_err(HttpError::BindError)?;
            let http_addr = http_listener
                .local_addr()
                .map_err(|_| HttpError::LocalAddrError)?;
            info!("HTTP server listening on {} for ACME challenges", http_addr);
            tokio::spawn(async move {
                if let Err(e) = serve(http_listener, http_app.into_make_service()).await {
                    error!("HTTP server error: {}", e);
                }
            });

            // Give the HTTP server a moment to start
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Start certificate generation in background - don't block main loop
            let acme_manager = acme_manager_opt.unwrap();
            let app_for_https = app.clone();
            tokio::spawn(async move {
                match tokio::time::timeout(
                    tokio::time::Duration::from_secs(300), // 5 minute timeout
                    acme_manager.get_certificate(),
                )
                .await
                {
                    Ok(Ok(tls_config)) => {
                        info!("Successfully got certificate, starting HTTPS server");
                        let rustls_config = RustlsConfig::from_config(Arc::new(tls_config));

                        match tokio::net::TcpListener::bind("0.0.0.0:443").await {
                            Ok(https_listener) => {
                                let https_addr = https_listener.local_addr().unwrap();
                                info!("HTTPS server listening on {}", https_addr);
                                let std_listener = https_listener.into_std().unwrap();
                                if let Err(e) =
                                    axum_server::from_tcp_rustls(std_listener, rustls_config)
                                        .serve(app_for_https.into_make_service())
                                        .await
                                {
                                    error!("HTTPS server error: {}", e);
                                }
                            }
                            Err(e) => {
                                error!("Failed to bind HTTPS listener: {}", e);
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Certificate generation failed: {}", e);
                        info!("Continuing with HTTP-only mode");
                    }
                    Err(_) => {
                        error!("Certificate generation timed out after 5 minutes");
                        info!("Continuing with HTTP-only mode");
                    }
                }
            });
        }

        let channel_map = RwLock::new(HashMap::<u64, Responder>::new());
        let uri_map = RwLock::new(HashMap::<u64, String>::new());
        let regular_cache = Arc::new(RwLock::new(HashMap::<String, CachedResponse>::new()));
        let htmx_cache = Arc::new(RwLock::new(HashMap::<String, CachedResponse>::new()));

        // Parse cache expiration from environment variable, default to never expire
        let cache_duration = env::var("EXPIRE_CACHE")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs);

        if let Some(cache_duration) = cache_duration {
            info!(
                "Cache expiration enabled: {} seconds",
                cache_duration.as_secs()
            );

            let _regular_cache_invalidation_handle = {
                let regular_cache = Arc::clone(&regular_cache);
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(cache_duration);
                    loop {
                        interval.tick().await;
                        debug!("invalidating regular response cache");
                        regular_cache.write().await.clear();
                    }
                })
            };

            let _htmx_cache_invalidation_handle = {
                let htmx_cache = Arc::clone(&htmx_cache);
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(cache_duration);
                    loop {
                        interval.tick().await;
                        debug!("invalidating htmx response cache");
                        htmx_cache.write().await.clear();
                    }
                })
            };
        } else {
            info!("Cache expiration disabled - cache will persist until restart");
        }

        loop {
            select! {
                msg = rx.recv() => {
                    let msg = match msg {
                        Some(msg) => msg,
                        None => {
                            warn!("HTTP request channel closed, recreating channel");
                            let (new_tx, new_rx) = tokio::sync::mpsc::channel::<RequestMessage>(10);
                            rx = new_rx;

                            // Update the sender in the existing shared app_state
                            *app_state.sender.write().await = new_tx;
                            continue;
                        }
                    };
                    debug!("Processing request {} {} with id: {}", msg.method, msg.uri, msg.id);
                    debug!("headers: {:?}", msg.headers);
                    if let Some(ref body) = msg.body {
                        match String::from_utf8(body.to_vec()) {
                            Ok(body_str) => debug!("body as string: {}", body_str),
                            Err(_) => debug!("body (non-UTF8): {:?}", body),
                        }
                    } else {
                        debug!("body: None");
                    }

                    let request_result = async {
                        if msg.method == Method::GET {
                            let is_htmx = msg.headers.contains_key("hx-request");
                            let cache_to_use = if is_htmx { &htmx_cache } else { &regular_cache };

                            let cache_read = cache_to_use.read().await;
                            if let Some(cached) = cache_read.get(&msg.uri.to_string()) {
                                // Check expiration only if cache_duration is set
                                let should_serve = if let Some(cache_duration) = cache_duration {
                                    !cached.is_expired(cache_duration)
                                } else {
                                    true // Never expire if no duration set
                                };

                                if should_serve {
                                    let cache_type = if is_htmx { "HTMX" } else { "regular" };
                                    debug!("serving cached {} response for {}", cache_type, msg.uri);
                                    let cached_response = cached.to_response()?;
                                    let _ = msg.resp.send(Ok(cached_response));
                                    return Ok(());
                                }
                            }
                            drop(cache_read);
                        }

                        channel_map.write().await.insert(msg.id, msg.resp);
                        uri_map.write().await.insert(msg.id, msg.uri.to_string());
                        let mut slab = NounSlab::new();

                        let id = Atom::from_value(&mut slab, msg.id)
                            .map_err(|e| HttpError::AtomCreationError(e.to_string()))?;
                        let uri = Atom::from_value(&mut slab, msg.uri.to_string())
                            .map_err(|e| HttpError::AtomCreationError(e.to_string()))?;
                        let method = Atom::from_value(&mut slab, msg.method.to_string())
                            .map_err(|e| HttpError::AtomCreationError(e.to_string()))?;

                        let mut headers = D(0);
                        for (k, v) in msg.headers {
                            let key = k.ok_or(HttpError::InvalidHeaderName)?.as_str().to_string();
                            let val = v.to_str().map_err(HttpError::InvalidHeaderValue)?.to_string();
                            let k_atom = Atom::from_value(&mut slab, key)
                                .map_err(|e| HttpError::AtomCreationError(e.to_string()))?;
                            let v_atom = Atom::from_value(&mut slab, val)
                                .map_err(|e| HttpError::AtomCreationError(e.to_string()))?;
                            let header_cell = T(&mut slab, &[k_atom.as_noun(), v_atom.as_noun()]);
                            headers = T(&mut slab, &[header_cell, headers]);
                        }

                        let body: crate::Noun = {
                            if let Some(bod) = msg.body {
                                let ato = Atom::from_bytes(&mut slab, &bod).as_noun();
                                let len: u64 = bod.len().try_into().map_err(|_| HttpError::BodyLengthConversion)?;
                                T(&mut slab, &[D(0), D(len), ato])
                            } else {
                                D(0)
                            }
                        };

                        let poke = T(
                            &mut slab,
                            &[D(tas!(b"req")), id.as_noun(), uri.as_noun(), method.as_noun(), headers, body],
                        );
                        debug!("poking kernel with request for {}", msg.uri);
                        slab.set_root(poke);

                        let wire = HttpWire::Request.to_wire();
                        let poke_result = handle.poke(wire, slab).await?;
                        debug!("poke result for {}: {:?}", msg.uri, poke_result);

                        if let PokeResult::Nack = poke_result {
                            error!("Kernel nacked the request for {}", msg.uri);
                            let resp_tx = channel_map.write().await.remove(&msg.id)
                                .ok_or(HttpError::ResponseChannelNotFound(msg.id))?;
                            uri_map.write().await.remove(&msg.id);
                            let _ = resp_tx.send(Err(StatusCode::BAD_REQUEST));
                        }

                        Ok::<(), HttpError>(())
                    }.await;

                    if let Err(e) = request_result {
                        error!("Error processing HTTP request: {}", e);
                        // Try to send error response if we still have the channel
                        if let Some(resp_tx) = channel_map.write().await.remove(&msg.id) {
                            uri_map.write().await.remove(&msg.id);
                            let _ = resp_tx.send(Err(StatusCode::INTERNAL_SERVER_ERROR));
                        }
                    }
                }
                effect = handle.next_effect() => {
                    let effect_result = async {
                        let slab = match effect {
                            Ok(slab) => {
                                debug!("received effect from kernel");
                                slab
                            }
                            Err(e) => {
                                error!("Error receiving effect in HTTP driver: {:?}", e);
                                return Ok(());
                            }
                        };
                        let effect = unsafe { slab.root() };
                        let res_list = effect.as_cell()?;

                        let head_tag = res_list.head().as_atom()?;
                        let tag_val = head_tag.as_u64().map_err(|e| HttpError::AtomCreationError(e.to_string()))?;
                        if tag_val != tas!(b"res") && tag_val != tas!(b"cache") && tag_val != tas!(b"htmx") && tag_val != tas!(b"h-cache") {
                            debug!("http: not an HTTP response effect, skipping. Got tag: {:?}", head_tag);
                            return Ok(());
                        }

                        debug!("processing HTTP response effect");
                        let mut res = res_list.tail().as_cell()?;
                        let id = res.head().as_atom()?.as_u64()
                            .map_err(|e| HttpError::AtomCreationError(e.to_string()))?;
                        debug!("HTTP response for request id: {}", id);

                        res = res.tail().as_cell()?;
                        let status_code = res
                            .head()
                            .as_atom()?
                            .direct()
                            .expect("not a valid status code!")
                            .data();
                        debug!("HTTP response status code: {}", status_code);

                        let mut header_list = res.tail().as_cell()?.head();
                        let mut header_vec: Vec<(String, String)> = Vec::new();
                        loop {
                            if header_list.is_atom() {
                                break;
                            } else {
                                let header = header_list.as_cell()?.head().as_cell()?;
                                let key_vec = header.head().as_atom()?;
                                let val_vec = header.tail().as_atom()?;

                                if let Ok(key) = key_vec.to_bytes_until_nul() {
                                    if let Ok(val) = val_vec.to_bytes_until_nul() {
                                        let key_str = String::from_utf8(key)?;
                                        let val_str = String::from_utf8(val)?;
                                        debug!("HTTP response header: {}: {}", key_str, val_str);
                                        header_vec.push((key_str, val_str));
                                        header_list = header_list.as_cell()?.tail();
                                    } else {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }
                        }

                        let maybe_body = res.tail().as_cell()?.tail();

                        let body: Option<Bytes> = {
                            if maybe_body.is_cell() {
                                let body_octs = maybe_body.as_cell()?.tail().as_cell()?;
                                let body_len = body_octs
                                    .head()
                                    .as_atom()?
                                    .direct()
                                    .expect("body len")
                                    .data();
                                let len: usize = body_len.try_into().map_err(|_| HttpError::BodyLengthConversion)?;
                                let mut body_vec: Vec<u8> = vec![0; len];
                                let body_atom = body_octs.tail().as_atom()?;

                                // Use lossy conversion to handle invalid UTF-8 gracefully
                                let body_bytes = match body_atom.to_bytes_until_nul() {
                                    Ok(bytes) => bytes,
                                    Err(e) => {
                                        error!("Failed to convert body atom to bytes: {}", e);
                                        // Try to get raw bytes from the atom instead
                                        let raw_bytes = body_atom.to_ne_bytes();
                                        let raw_size = std::cmp::min(len, raw_bytes.len());
                                        raw_bytes[..raw_size].to_vec()
                                    }
                                };

                                body_vec.copy_from_slice(&body_bytes[..std::cmp::min(body_bytes.len(), len)]);
                                let bytes = Bytes::from(body_vec);

                                // Log the response body as string if possible, using lossy conversion
                                let body_str = String::from_utf8_lossy(&bytes);
                                debug!("HTTP response body as string: {}", body_str);
                                if body_str.contains('\u{FFFD}') {
                                    warn!("Note: Response body contained invalid UTF-8 sequences (replaced with)");
                                }

                                Some(bytes)
                            } else {
                                debug!("HTTP response has no body");
                                None
                            }
                        };

                        let resp = if let Ok(status) = StatusCode::from_u16(status_code as u16) {
                            debug!("Building HTTP response with status: {}", status);
                            let res_builder = ResponseBuilder {
                                status_code: status,
                                headers: header_vec.clone(),
                                body: body.clone(),
                            };

                            let mut res = Response::builder().status(res_builder.status_code);

                            for (k, v) in &res_builder.headers {
                                res = res.header(k, v);
                            }

                            let response_body = res_builder.body
                                .map(Body::from)
                                .unwrap_or_else(Body::empty);

                            let response = res.body(response_body).map_err(HttpError::ResponseBuildError)?;

                            // Cache logic - determine which cache to use based on effect type
                            if status == StatusCode::OK {
                                if let Some(request_uri) = uri_map.read().await.get(&id).cloned() {
                                    let cached_response = CachedResponse::new(status, header_vec.clone(), body.clone());
                                    if tag_val == tas!(b"htmx") || tag_val == tas!(b"h-cache") {
                                        debug!("caching HTMX response for {} (htmx or h-cache effect)", request_uri);
                                        htmx_cache.write().await.insert(request_uri, cached_response);
                                    } else {
                                        debug!("caching regular response for {} (res or cache effect)", request_uri);
                                        regular_cache.write().await.insert(request_uri, cached_response);
                                    }
                                }
                            }

                            Ok(response)
                        } else {
                            error!("http: not a valid status code: {}", status_code);
                            error!("http: res: {:?}", res);
                            Err(StatusCode::INTERNAL_SERVER_ERROR)
                        };

                        if tag_val == tas!(b"res") || tag_val == tas!(b"htmx") {
                            let resp_tx = channel_map.write().await.remove(&id)
                                .ok_or(HttpError::ResponseChannelNotFound(id))?;
                            uri_map.write().await.remove(&id);
                            debug!("Sending response back to client for request id: {}", id);
                            let _ = resp_tx.send(resp);
                        }

                        Ok::<(), HttpError>(())
                    }.await;

                    if let Err(e) = effect_result {
                        error!("Error processing HTTP effect: {}", e);
                    }
                }
            }
        }
    })
}

async fn nockvm_handler(
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    debug!("Received request: {} {}", method, uri);
    debug!("Headers: {:?}", headers);
    debug!("Body length: {}", body.len());

    let (mut resp_tx, mut resp_rx) = oneshot::channel::<Result<Response, StatusCode>>();
    let opt_body: Option<axum::body::Bytes> = {
        if body.is_empty() {
            None
        } else {
            Some(body)
        }
    };

    let request_id = get_id();

    // Try to send the message, with a retry for closed channels
    let mut retry_count = 0;
    const MAX_RETRIES: usize = 3;

    loop {
        let msg = RequestMessage {
            id: request_id,
            uri: uri.clone(),
            method: method.clone(),
            headers: headers.clone(),
            body: opt_body.clone(),
            resp: resp_tx,
        };

        // Get the current sender from shared state (it might have been recreated)
        let send_result = {
            let sender_guard = state.sender.read().await;
            sender_guard.send(msg).await
        };

        match send_result {
            Ok(()) => break,
            Err(e) => {
                error!(
                    "Failed to send request (attempt {}): {}",
                    retry_count + 1,
                    e
                );

                // For channel closed errors, retry after a short delay
                if matches!(e, tokio::sync::mpsc::error::SendError(_)) {
                    retry_count += 1;
                    if retry_count >= MAX_RETRIES {
                        error!(
                            "Max retries reached for closed channel, returning service unavailable"
                        );
                        return Err(StatusCode::SERVICE_UNAVAILABLE);
                    }

                    warn!(
                        "Channel closed, waiting for recreation (retry {}/{})",
                        retry_count, MAX_RETRIES
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                    // Need to create a new oneshot channel for the retry
                    let (new_resp_tx, new_resp_rx) =
                        oneshot::channel::<Result<Response, StatusCode>>();
                    resp_tx = new_resp_tx;
                    resp_rx = new_resp_rx;
                    continue;
                } else {
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
    }

    // Await the response
    match resp_rx.await {
        Ok(result) => {
            debug!(
                "Received response for {}: {:?}",
                uri,
                result.as_ref().map(|r| r.status())
            );
            result
        }
        Err(e) => {
            error!("Failed to receive response for {}: {}", uri, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Default favicon handler
///
/// Renders a simple black circle with a white circle in the center as an SVG.
async fn favicon_handler() -> Response {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="\#2563eb"/><circle cx="8" cy="8" r="3" fill="white"/></svg>"#;

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "image/svg+xml")
        .header("cache-control", "public, max-age=86400")
        .body(Body::from(svg))
        .unwrap()
}
