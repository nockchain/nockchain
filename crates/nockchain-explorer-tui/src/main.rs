// Allow architectural patterns
#![allow(clippy::large_enum_variant)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::needless_borrows_for_generic_args)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::or_fun_call)]
#![allow(clippy::explicit_counter_loop)]
#![allow(clippy::vec_init_then_push)]
#![allow(clippy::unwrap_or_default)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{env, io};

use anyhow::{anyhow, Result};
use arboard::Clipboard;
use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use clap::Parser;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use nockapp_grpc_proto::pb::common::v1::{self as pb_common, Base58Hash, PageRequest};
use nockapp_grpc_proto::pb::public::v2::nockchain_block_service_client::NockchainBlockServiceClient;
use nockapp_grpc_proto::pb::public::v2::nockchain_metrics_service_client::NockchainMetricsServiceClient;
use nockapp_grpc_proto::pb::public::v2::{
    get_block_details_request, get_block_details_response, get_blocks_response,
    get_explorer_metrics_response, get_transaction_block_response,
    get_transaction_details_response, transaction_details, transaction_output, BlockDetails,
    BlockEntry, ExplorerMetrics, GetBlockDetailsRequest, GetBlocksRequest,
    GetExplorerMetricsRequest, GetTransactionBlockRequest, GetTransactionDetailsRequest,
    TransactionBlockData, TransactionDetails as RpcTransactionDetails,
};
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::Hash as Tip5Hash;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tonic::Request;
use tracing::{info, trace, warn};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};
use tracing_tracy::TracyLayer;

#[derive(Parser, Debug)]
#[command(name = "nockchain-explorer-tui")]
#[command(about = "Block Explorer TUI for Nockchain", long_about = None)]
struct Args {
    /// gRPC server URI (e.g., http://localhost:50051)
    #[arg(short, long, default_value = "http://localhost:50051")]
    server: String,

    /// Fail immediately if cannot connect to server (old behavior)
    #[arg(long)]
    fail_fast: bool,
}

#[derive(Debug, Clone)]
enum View {
    BlocksList,
    TransactionsList,
    WalletsList,
    Metrics,
    BlockDetails(usize), // index in blocks list
    TransactionDetails { block_idx: usize, tx_idx: usize },
    TransactionSearch,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ConnectionStatus {
    NeverConnected,
    Connected,
    Disconnected,
    Reconnecting,
}

const PAGE_JUMP: usize = 20;
const EMPTY_CACHE_BACKOFF: Duration = Duration::from_secs(30);
const ERROR_REFRESH_BACKOFF: Duration = Duration::from_secs(5);
const WALLET_INDEX_CHUNK: usize = 64;
const NICKS_PER_NOCK: u64 = 65_536;
const SPINNER_FRAMES: [&str; 4] = ["◴", "◷", "◶", "◵"];
const SPINNER_COLORS: [Color; 6] = [
    Color::Red,
    Color::Yellow,
    Color::Green,
    Color::Cyan,
    Color::Blue,
    Color::Magenta,
];

struct App {
    client: Option<NockchainBlockServiceClient<tonic::transport::Channel>>,
    blocks: Vec<BlockEntry>,
    cached_blocks: BTreeMap<u64, BlockEntry>,
    current_height: u64,
    list_state: ListState,
    view: View,
    next_page_token: Option<String>,
    has_more_pages: bool,
    loading: bool,
    error_message: Option<String>,
    status_message: Option<String>,
    clear_status_on_input: bool,
    last_refresh: Instant,
    next_allowed_refresh: Instant,
    tx_search_input: String,
    tx_search_result: Option<TxSearchResult>,
    server_uri: String,
    previous_view: Option<View>,
    tx_list_state: ListState,
    tx_detail: Option<TxDetailState>,
    transactions: Vec<TransactionSummary>,
    transaction_index: HashMap<(usize, usize), usize>,
    tx_overview_state: ListState,
    wallets: Vec<WalletSummary>,
    wallet_map: HashMap<String, WalletTally>,
    wallet_list_state: ListState,
    wallet_indexed_txs: HashSet<String>,
    wallet_inflight_txs: HashSet<String>,
    wallet_indexing: bool,
    wallet_index_message: Option<String>,
    wallet_worker_tx: UnboundedSender<WalletWorkerCommand>,
    wallet_worker_rx: UnboundedReceiver<WalletWorkerResult>,
    wallet_sort_key: WalletSortKey,
    wallet_sort_ascending: bool,
    wallet_index_highest_synced: u64,
    clipboard: Option<Clipboard>,
    block_focus: BlockDetailsFocus,
    full_block_details: HashMap<u64, BlockDetails>,
    loading_block_details: Option<u64>,
    metrics_client: Option<NockchainMetricsServiceClient<tonic::transport::Channel>>,
    metrics_data: Option<ExplorerMetrics>,
    metrics_error: Option<String>,
    last_user_action: Instant,
    help_scroll: u16,
    help_max_scroll: u16,
    active_tab: usize,
    busy: bool,
    spinner_index: usize,
    shutdown_flag: Arc<AtomicBool>,

    // Connection state
    connection_status: ConnectionStatus,
    last_successful_connection: Option<Instant>,
    last_connection_attempt: Instant,
    last_connection_error: Option<String>,
    #[allow(dead_code)]
    fail_fast: bool,

    // Prefetch state - background loading of block details to avoid loading screens
    prefetch_queue: VecDeque<u64>,
    prefetch_in_progress: bool,

    // Priority prefetch queue - for blocks user navigates to (processed first)
    priority_prefetch_queue: VecDeque<u64>,
    // Deferred page load request (non-blocking)
    request_next_page: bool,
    // Whether scroll-triggered preloading is enabled (env var EXPLORER_SCROLL_PRELOAD)
    scroll_preload_enabled: bool,
}

#[derive(Debug, Clone)]
enum TxSearchResult {
    Found(TransactionBlockData),
    Pending,
    NotFound,
    Error(String),
}

#[derive(Debug, Clone)]
enum TxDetailStatus {
    Confirmed(RpcTransactionDetails),
    Pending,
    NotFound,
    Error(String),
}

#[derive(Debug, Clone)]
struct TxDetailState {
    tx_id: String,
    status: TxDetailStatus,
    pane_focus: TxDetailPane,
    inputs_scroll: u16,
    outputs_scroll: u16,
}

#[derive(Debug, Clone)]
struct TransactionSummary {
    tx_id: String,
    block_height: u64,
    block_idx: usize,
    tx_idx: usize,
}

#[derive(Debug, Clone)]
struct WalletSummary {
    address: String,
    total_received: u64,
    total_sent: u64,
    tx_count: usize,
}

#[derive(Debug, Default, Clone)]
struct WalletTally {
    total_received: u64,
    total_sent: u64,
    tx_count: usize,
}

#[derive(Debug, Clone)]
struct WalletIndexTask {
    tx_id: String,
    block_height: u64,
}

#[derive(Debug, Clone)]
struct WalletDelta {
    address: String,
    received: u64,
    sent: u64,
    tx_count: usize,
}

#[derive(Debug)]
enum WalletWorkerCommand {
    IndexTransactions {
        range_start: u64,
        range_end: u64,
        tasks: Vec<WalletIndexTask>,
    },
}

#[derive(Debug)]
enum WalletWorkerResult {
    ChunkComplete {
        tx_ids: Vec<String>,
        deltas: Vec<WalletDelta>,
        range_start: u64,
        range_end: u64,
    },
    Error {
        tx_ids: Vec<String>,
        message: String,
    },
    Status(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalletSortKey {
    Balance,
    TotalReceived,
    TotalSent,
    TxCount,
}

impl WalletSortKey {
    fn label(self) -> &'static str {
        match self {
            WalletSortKey::Balance => "Balance",
            WalletSortKey::TotalReceived => "Total Received",
            WalletSortKey::TotalSent => "Total Sent",
            WalletSortKey::TxCount => "Transactions",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockDetailsFocus {
    Block,
    Transactions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TxDetailPane {
    Inputs,
    Outputs,
}

impl App {
    #[tracing::instrument(name = "tui.block_explorer.app_new")]
    async fn new(server_uri: String, fail_fast: bool) -> Result<Self> {
        // Try to connect, but don't fail if we can't (unless fail_fast is set)
        let (client, connection_status, connection_error) =
            match NockchainBlockServiceClient::connect(server_uri.clone()).await {
                Ok(client) => (Some(client), ConnectionStatus::Connected, None),
                Err(e) => {
                    if fail_fast {
                        return Err(anyhow!("Failed to connect to gRPC server").context(e));
                    }
                    warn!("Initial connection failed: {}, will retry in background", e);
                    (None, ConnectionStatus::NeverConnected, Some(e.to_string()))
                }
            };
        let metrics_client =
            (NockchainMetricsServiceClient::connect(server_uri.clone()).await).ok();

        let (wallet_cmd_tx, wallet_cmd_rx) = mpsc::unbounded_channel();
        let (wallet_res_tx, wallet_res_rx) = mpsc::unbounded_channel();
        let wallet_worker_uri = server_uri.clone();
        tokio::spawn(async move {
            wallet_index_worker(wallet_worker_uri, wallet_cmd_rx, wallet_res_tx).await;
        });

        let mut app = Self {
            client,
            blocks: Vec::new(),
            cached_blocks: BTreeMap::new(),
            current_height: 0,
            list_state: ListState::default(),
            view: View::BlocksList,
            next_page_token: None,
            has_more_pages: true,
            loading: false,
            error_message: connection_error.clone(),
            status_message: None,
            clear_status_on_input: false,
            last_refresh: Instant::now(),
            next_allowed_refresh: Instant::now(),
            tx_search_input: String::new(),
            tx_search_result: None,
            server_uri,
            previous_view: None,
            tx_list_state: ListState::default(),
            tx_detail: None,
            transactions: Vec::new(),
            transaction_index: HashMap::new(),
            tx_overview_state: ListState::default(),
            wallets: Vec::new(),
            wallet_map: HashMap::new(),
            wallet_list_state: ListState::default(),
            wallet_indexed_txs: HashSet::new(),
            wallet_inflight_txs: HashSet::new(),
            wallet_indexing: false,
            wallet_index_message: None,
            wallet_worker_tx: wallet_cmd_tx,
            wallet_worker_rx: wallet_res_rx,
            wallet_sort_key: WalletSortKey::Balance,
            wallet_sort_ascending: false,
            wallet_index_highest_synced: 0,
            clipboard: Clipboard::new().ok(),
            block_focus: BlockDetailsFocus::Block,
            full_block_details: HashMap::new(),
            loading_block_details: None,
            metrics_client,
            metrics_data: None,
            metrics_error: None,
            last_user_action: Instant::now(),
            help_scroll: 0,
            help_max_scroll: 0,
            active_tab: 0,
            busy: false,
            spinner_index: 0,
            shutdown_flag: Arc::new(AtomicBool::new(false)),

            // Connection state
            connection_status,
            last_successful_connection: if connection_status == ConnectionStatus::Connected {
                Some(Instant::now())
            } else {
                None
            },
            last_connection_attempt: Instant::now(),
            last_connection_error: connection_error,
            fail_fast,

            // Prefetch state
            prefetch_queue: VecDeque::new(),
            prefetch_in_progress: false,

            // Priority prefetch queue
            priority_prefetch_queue: VecDeque::new(),
            request_next_page: false,
            // Scroll-triggered preloading is opt-in via env var
            scroll_preload_enabled: env::var("EXPLORER_SCROLL_PRELOAD").is_ok(),
        };

        // Only try to load blocks if connected
        if app.connection_status == ConnectionStatus::Connected {
            let _ = app.load_blocks(None).await; // Don't fail if this errors
        }

        Ok(app)
    }

    fn set_view(&mut self, view: View) {
        let tab = match &view {
            View::BlocksList | View::BlockDetails(_) => 0,
            View::TransactionsList | View::TransactionDetails { .. } | View::TransactionSearch => 1,
            View::WalletsList => 2,
            View::Metrics => 3,
            View::Help => self.active_tab,
        };
        self.active_tab = tab;
        self.view = view;
    }

    fn activate_tab(&mut self, tab: usize) {
        match tab % 4 {
            0 => self.set_view(View::BlocksList),
            1 => {
                self.set_view(View::TransactionsList);
                if self.transactions.is_empty() {
                    self.status_message = Some("No transactions cached yet".into());
                    self.clear_status_on_input = true;
                }
            }
            2 => {
                self.set_view(View::WalletsList);
                if self.wallets.is_empty() {
                    self.status_message =
                        Some("Wallet index empty; waiting for cached data…".into());
                    self.clear_status_on_input = true;
                }
            }
            3 => {
                self.set_view(View::Metrics);
            }
            _ => {}
        }
    }

    fn cycle_tabs(&mut self, delta: i32) {
        let total_tabs = 4;
        let idx = (self.active_tab as i32 + delta).rem_euclid(total_tabs as i32) as usize;
        self.activate_tab(idx);
    }

    #[tracing::instrument(
        name = "tui.block_explorer.load_blocks",
        skip(self),
        fields(page_token = tracing::field::Empty)
    )]
    async fn load_blocks(&mut self, page_token: Option<String>) -> Result<()> {
        let span = tracing::Span::current();
        if let Some(ref token) = page_token {
            span.record("page_token", &tracing::field::display(token.as_str()));
        } else {
            span.record("page_token", &tracing::field::display("tip"));
        }
        let Some(ref mut client) = self.client else {
            self.error_message = Some("Not connected to server".into());
            self.defer_auto_refresh(ERROR_REFRESH_BACKOFF);
            return Ok(());
        };

        self.loading = true;
        self.error_message = None;

        let request = GetBlocksRequest {
            page: Some(PageRequest {
                page_token: page_token.clone().unwrap_or_default(),
                client_page_items_limit: 50,
                max_bytes: 0,
            }),
        };

        match client.get_blocks(Request::new(request)).await {
            Ok(response) => {
                // Mark as connected on successful response
                if self.connection_status != ConnectionStatus::Connected {
                    self.connection_status = ConnectionStatus::Connected;
                    self.last_successful_connection = Some(Instant::now());
                }

                let resp = response.into_inner();
                match resp.result {
                    Some(get_blocks_response::Result::Blocks(blocks_data)) => {
                        if !blocks_data.blocks.is_empty() {
                            self.integrate_blocks(blocks_data.blocks);
                        } else if page_token.is_some() {
                            self.has_more_pages = false;
                            self.next_page_token = None;
                        } else if self.blocks.is_empty() {
                            self.cached_blocks.clear();
                            self.rebuild_blocks(None);
                        }

                        self.current_height = blocks_data.current_height;
                        self.record_refresh();
                        if self.current_height == 0 {
                            self.status_message =
                                Some("Waiting for server to sync with the network…".into());
                            self.clear_status_on_input = false;
                            self.defer_auto_refresh(EMPTY_CACHE_BACKOFF);
                        } else if matches!(
                            self.status_message.as_deref(),
                            Some(msg) if msg.contains("Waiting for server to sync")
                        ) {
                            self.status_message = None;
                        }
                    }
                    Some(get_blocks_response::Result::Error(err)) => {
                        self.error_message = Some(format!("API Error: {}", err.message));
                        self.defer_auto_refresh(ERROR_REFRESH_BACKOFF);
                    }
                    None => {
                        self.error_message = Some("Empty response from server".to_string());
                        self.defer_auto_refresh(ERROR_REFRESH_BACKOFF);
                    }
                }
            }
            Err(e) => {
                // Mark as disconnected on error
                self.connection_status = ConnectionStatus::Disconnected;
                self.last_connection_error = Some(e.to_string());
                self.error_message = Some(format!("gRPC Error: {}", e));
                self.defer_auto_refresh(ERROR_REFRESH_BACKOFF);
            }
        }

        self.loading = false;
        Ok(())
    }

    #[tracing::instrument(name = "tui.block_explorer.load_metrics", skip(self))]
    async fn load_metrics(&mut self) -> Result<()> {
        // Ensure we have a metrics client
        if self.metrics_client.is_none() {
            match NockchainMetricsServiceClient::connect(self.server_uri.clone()).await {
                Ok(client) => self.metrics_client = Some(client),
                Err(e) => {
                    self.metrics_error = Some(format!("Metrics connect error: {}", e));
                    return Ok(());
                }
            }
        }

        let Some(ref mut client) = self.metrics_client else {
            return Ok(());
        };

        match client
            .get_explorer_metrics(Request::new(GetExplorerMetricsRequest {}))
            .await
        {
            Ok(response) => {
                let resp = response.into_inner();
                match resp.result {
                    Some(get_explorer_metrics_response::Result::Metrics(metrics)) => {
                        self.metrics_data = Some(metrics);
                        self.metrics_error = None;
                    }
                    Some(get_explorer_metrics_response::Result::Error(e)) => {
                        self.metrics_error = Some(format!("Metrics error: {}", e.message));
                    }
                    None => {
                        self.metrics_error = Some("Empty metrics response".into());
                    }
                }
            }
            Err(e) => {
                self.metrics_error = Some(format!("Metrics gRPC error: {}", e));
                self.metrics_client = None;
            }
        }

        Ok(())
    }

    #[tracing::instrument(name = "tui.block_explorer.load_next_page", skip(self))]
    async fn load_next_page(&mut self) -> Result<()> {
        if let Some(token) = self.next_page_token.clone() {
            self.load_blocks(Some(token)).await?;
        }
        Ok(())
    }

    #[tracing::instrument(name = "tui.block_explorer.refresh", skip(self))]
    async fn refresh(&mut self) -> Result<()> {
        self.load_blocks(None).await
    }

    #[tracing::instrument(
        name = "tui.block_explorer.load_full_block_details",
        skip(self),
        fields(height = tracing::field::Empty)
    )]
    async fn load_full_block_details(&mut self, height: u64) -> Result<()> {
        tracing::Span::current().record("height", &tracing::field::display(height));

        if self.full_block_details.contains_key(&height) {
            return Ok(());
        }

        let Some(ref mut client) = self.client else {
            return Err(anyhow!("Not connected to server"));
        };

        self.loading_block_details = Some(height);

        let request = GetBlockDetailsRequest {
            selector: Some(get_block_details_request::Selector::Height(height)),
        };

        match client.get_block_details(Request::new(request)).await {
            Ok(response) => {
                if self.connection_status != ConnectionStatus::Connected {
                    self.connection_status = ConnectionStatus::Connected;
                    self.last_successful_connection = Some(Instant::now());
                }

                let resp = response.into_inner();
                match resp.result {
                    Some(get_block_details_response::Result::Details(details)) => {
                        self.full_block_details.insert(height, details);
                    }
                    Some(get_block_details_response::Result::Error(e)) => {
                        self.error_message = Some(format!("Error: {}", e.message));
                    }
                    None => {
                        self.error_message = Some("No response from server".into());
                    }
                }
            }
            Err(e) => {
                crash_happy_check(&format!("load_full_block_details(height={})", height), &e);
                self.connection_status = ConnectionStatus::Disconnected;
                self.last_connection_error = Some(e.to_string());
                self.error_message = Some(format!("gRPC Error: {}", e));
            }
        }

        self.loading_block_details = None;
        Ok(())
    }

    #[tracing::instrument(name = "tui.block_explorer.attempt_reconnect", skip(self))]
    async fn attempt_reconnect(&mut self) -> Result<()> {
        self.last_connection_attempt = Instant::now();
        self.connection_status = ConnectionStatus::Reconnecting;

        match NockchainBlockServiceClient::connect(self.server_uri.clone()).await {
            Ok(client) => {
                self.client = Some(client);
                self.metrics_client =
                    NockchainMetricsServiceClient::connect(self.server_uri.clone())
                        .await
                        .ok();
                self.connection_status = ConnectionStatus::Connected;
                self.last_successful_connection = Some(Instant::now());
                self.last_connection_error = None;
                self.status_message = Some("Connected to server!".into());
                trace!("Reconnected to server at {}", self.server_uri);

                // Try to load initial blocks
                let _ = self.load_blocks(None).await;
                Ok(())
            }
            Err(e) => {
                self.connection_status = if self.last_successful_connection.is_some() {
                    ConnectionStatus::Disconnected
                } else {
                    ConnectionStatus::NeverConnected
                };
                self.last_connection_error = Some(e.to_string());
                Err(anyhow!("Reconnection failed: {}", e))
            }
        }
    }

    fn should_retry_connection(&self) -> bool {
        matches!(
            self.connection_status,
            ConnectionStatus::NeverConnected | ConnectionStatus::Disconnected
        ) && self.last_connection_attempt.elapsed() >= Duration::from_secs(5)
    }

    #[tracing::instrument(
        name = "tui.block_explorer.search_transaction",
        skip(self),
        fields(query_len = tracing::field::Empty)
    )]
    async fn search_transaction(&mut self, tx_id: &str) -> Result<()> {
        let Some(ref mut client) = self.client else {
            self.tx_search_result = Some(TxSearchResult::Error("Not connected to server".into()));
            return Ok(());
        };

        self.loading = true;
        self.error_message = None;
        self.tx_search_result = None;

        let trimmed = tx_id.trim();
        tracing::Span::current().record("query_len", &tracing::field::display(trimmed.len()));
        if trimmed.is_empty() {
            self.tx_search_result = Some(TxSearchResult::Error(
                "Please enter at least one character".into(),
            ));
            self.loading = false;
            return Ok(());
        }

        let request = GetTransactionBlockRequest {
            tx_id: Some(Base58Hash {
                hash: trimmed.to_string(),
            }),
        };

        match client.get_transaction_block(Request::new(request)).await {
            Ok(response) => {
                // Mark as connected on successful response
                if self.connection_status != ConnectionStatus::Connected {
                    self.connection_status = ConnectionStatus::Connected;
                    self.last_successful_connection = Some(Instant::now());
                }

                let resp = response.into_inner();
                self.tx_search_result = Some(Self::map_tx_response(resp.result));
            }
            Err(e) => {
                // Mark as disconnected on error
                self.connection_status = ConnectionStatus::Disconnected;
                self.last_connection_error = Some(e.to_string());
                self.tx_search_result = Some(TxSearchResult::Error(format!("gRPC Error: {}", e)));
            }
        }

        self.loading = false;
        Ok(())
    }

    #[tracing::instrument(
        name = "tui.block_explorer.open_transaction_detail",
        skip(self),
        fields(tx_id = tracing::field::Empty)
    )]
    async fn open_transaction_detail(&mut self, block_idx: usize, tx_idx: usize) -> Result<()> {
        let Some(ref mut client) = self.client else {
            self.error_message = Some("Not connected to server".into());
            return Ok(());
        };

        let tx_id = {
            let block = self
                .blocks
                .get(block_idx)
                .ok_or_else(|| anyhow!("Block index out of range"))?;
            block
                .tx_ids
                .get(tx_idx)
                .ok_or_else(|| anyhow!("Transaction index out of range"))?
                .hash
                .clone()
        };
        tracing::Span::current().record("tx_id", &tracing::field::display(tx_id.as_str()));

        self.loading = true;
        self.error_message = None;

        let request = GetTransactionDetailsRequest {
            tx_id: Some(Base58Hash {
                hash: tx_id.clone(),
            }),
        };

        match client.get_transaction_details(Request::new(request)).await {
            Ok(response) => {
                // Mark as connected on successful response
                if self.connection_status != ConnectionStatus::Connected {
                    self.connection_status = ConnectionStatus::Connected;
                    self.last_successful_connection = Some(Instant::now());
                }

                let resp = response.into_inner();
                self.tx_detail = Some(TxDetailState {
                    tx_id: tx_id.clone(),
                    status: Self::map_tx_detail_response(resp.result),
                    pane_focus: TxDetailPane::Inputs,
                    inputs_scroll: 0,
                    outputs_scroll: 0,
                });
                self.set_view(View::TransactionDetails { block_idx, tx_idx });
                self.block_focus = BlockDetailsFocus::Transactions;
                self.set_transaction_overview_selection(block_idx, tx_idx);
            }
            Err(e) => {
                crash_happy_check(&format!("open_transaction_detail(tx_id={})", tx_id), &e);
                // Mark as disconnected on error
                self.connection_status = ConnectionStatus::Disconnected;
                self.last_connection_error = Some(e.to_string());
                self.error_message = Some(format!("gRPC Error: {}", e));
            }
        }

        self.loading = false;
        Ok(())
    }

    fn map_tx_response(result: Option<get_transaction_block_response::Result>) -> TxSearchResult {
        match result {
            Some(get_transaction_block_response::Result::Block(block_data)) => {
                TxSearchResult::Found(block_data)
            }
            Some(get_transaction_block_response::Result::Pending(_)) => TxSearchResult::Pending,
            Some(get_transaction_block_response::Result::Error(err)) => {
                if err.message.contains("not found") {
                    TxSearchResult::NotFound
                } else {
                    TxSearchResult::Error(err.message)
                }
            }
            None => TxSearchResult::Error("Empty response".to_string()),
        }
    }

    fn map_tx_detail_response(
        result: Option<get_transaction_details_response::Result>,
    ) -> TxDetailStatus {
        match result {
            Some(get_transaction_details_response::Result::Details(details)) => {
                TxDetailStatus::Confirmed(details)
            }
            Some(get_transaction_details_response::Result::Pending(_)) => TxDetailStatus::Pending,
            Some(get_transaction_details_response::Result::Error(err)) => {
                if err.message.contains("not found") {
                    TxDetailStatus::NotFound
                } else {
                    TxDetailStatus::Error(err.message)
                }
            }
            None => TxDetailStatus::Error("Empty response".to_string()),
        }
    }

    fn selected_height(&self) -> Option<u64> {
        self.list_state
            .selected()
            .and_then(|idx| self.blocks.get(idx))
            .map(|b| b.height)
    }

    fn integrate_blocks(&mut self, new_blocks: Vec<BlockEntry>) {
        let preferred_height = self.selected_height();
        for block in new_blocks {
            self.cached_blocks.insert(block.height, block);
        }
        self.rebuild_blocks(preferred_height);
    }

    fn rebuild_blocks(&mut self, preferred_height: Option<u64>) {
        self.blocks = self
            .cached_blocks
            .iter()
            .rev()
            .map(|(_, block)| block.clone())
            .collect();

        let target_height = preferred_height.or_else(|| self.blocks.first().map(|b| b.height));
        if let Some(height) = target_height {
            if let Some(idx) = self.blocks.iter().position(|b| b.height == height) {
                self.list_state.select(Some(idx));
            } else if !self.blocks.is_empty() {
                self.list_state.select(Some(0));
            } else {
                self.list_state.select(None);
            }
        } else if !self.blocks.is_empty() {
            self.list_state.select(Some(0));
        } else {
            self.list_state.select(None);
        }

        self.update_pagination_tokens();

        if matches!(
            self.view,
            View::BlockDetails(_) | View::TransactionDetails { .. }
        ) {
            if let Some(idx) = self.list_state.selected() {
                self.set_view(View::BlockDetails(idx));
            } else {
                self.set_view(View::BlocksList);
            }
        }
        self.sync_tx_list_selection();
        self.rebuild_transactions_list();
        self.queue_prefetch_visible_blocks();
    }

    /// Queue visible blocks for background prefetch of full block details.
    /// This eliminates loading screens when navigating to block details.
    /// Only active when EXPLORER_SCROLL_PRELOAD env var is set.
    fn queue_prefetch_visible_blocks(&mut self) {
        if !self.scroll_preload_enabled {
            return;
        }
        // Queue first 15 blocks for prefetch (approximately what fits on screen)
        for block in self.blocks.iter().take(15) {
            if !self.full_block_details.contains_key(&block.height)
                && !self.prefetch_queue.contains(&block.height)
            {
                self.prefetch_queue.push_back(block.height);
            }
        }
    }

    /// Queue adjacent blocks for predictive prefetching when navigating BlockDetails.
    /// Only active when EXPLORER_SCROLL_PRELOAD env var is set.
    fn queue_adjacent_prefetch(&mut self, current_idx: usize) {
        if !self.scroll_preload_enabled {
            return;
        }
        // Prefetch next 2 and previous 2 blocks for smooth navigation
        for offset in [1i32, 2, -1, -2] {
            let adj_idx = if offset < 0 {
                current_idx.checked_sub(offset.unsigned_abs() as usize)
            } else {
                Some(current_idx + offset as usize)
            };
            if let Some(idx) = adj_idx {
                if let Some(block) = self.blocks.get(idx) {
                    if !self.full_block_details.contains_key(&block.height)
                        && !self.priority_prefetch_queue.contains(&block.height)
                    {
                        self.priority_prefetch_queue.push_back(block.height);
                    }
                }
            }
        }
    }

    fn update_pagination_tokens(&mut self) {
        if let Some(oldest) = self.blocks.last() {
            if oldest.height > 0 {
                self.next_page_token = Some(format!("{:x}", oldest.height - 1));
                self.has_more_pages = true;
            } else {
                self.next_page_token = None;
                self.has_more_pages = false;
            }
        } else {
            self.next_page_token = None;
            self.has_more_pages = false;
        }
    }

    fn rebuild_transactions_list(&mut self) {
        let mut summaries = Vec::new();
        let mut index = HashMap::new();
        for (block_idx, block) in self.blocks.iter().enumerate() {
            for (tx_idx, tx) in block.tx_ids.iter().enumerate() {
                let entry = TransactionSummary {
                    tx_id: tx.hash.clone(),
                    block_height: block.height,
                    block_idx,
                    tx_idx,
                };
                index.insert((block_idx, tx_idx), summaries.len());
                summaries.push(entry);
            }
        }
        self.transactions = summaries;
        self.transaction_index = index;
        if self.transactions.is_empty() {
            self.tx_overview_state.select(None);
        } else {
            let current = self
                .tx_overview_state
                .selected()
                .unwrap_or(0)
                .min(self.transactions.len() - 1);
            self.tx_overview_state.select(Some(current));
        }
        self.queue_wallet_index_work();
    }

    fn queue_wallet_index_work(&mut self) {
        if self.wallet_worker_tx.is_closed() {
            return;
        }
        let mut pending = Vec::new();
        for summary in self.transactions.clone() {
            if self.wallet_indexed_txs.contains(&summary.tx_id)
                || self.wallet_inflight_txs.contains(&summary.tx_id)
            {
                continue;
            }
            self.wallet_inflight_txs.insert(summary.tx_id.clone());
            pending.push(WalletIndexTask {
                tx_id: summary.tx_id.clone(),
                block_height: summary.block_height,
            });
            if pending.len() >= WALLET_INDEX_CHUNK {
                let chunk = std::mem::take(&mut pending);
                self.dispatch_wallet_chunk(chunk);
            }
        }
        if !pending.is_empty() {
            self.dispatch_wallet_chunk(pending);
        }
        self.wallet_indexing = !self.wallet_inflight_txs.is_empty();
        if self.wallet_indexing {
            self.wallet_index_message = Some(format!(
                "Indexing wallets… {} tx queued (max height {})",
                self.wallet_inflight_txs.len(),
                self.wallet_index_highest_synced
            ));
        }
    }

    fn dispatch_wallet_chunk(&mut self, tasks: Vec<WalletIndexTask>) {
        if tasks.is_empty() {
            return;
        }
        let (min_height, max_height) = tasks.iter().fold((u64::MAX, 0), |(min_h, max_h), task| {
            (min_h.min(task.block_height), max_h.max(task.block_height))
        });
        let ids: Vec<String> = tasks.iter().map(|t| t.tx_id.clone()).collect();
        let command = WalletWorkerCommand::IndexTransactions {
            range_start: if min_height == u64::MAX {
                0
            } else {
                min_height
            },
            range_end: max_height,
            tasks,
        };
        if self.wallet_worker_tx.send(command).is_err() {
            self.wallet_index_message = Some("Wallet indexer worker unavailable".to_string());
            // Revert inflight markers since worker did not accept work.
            for tx_id in ids {
                self.wallet_inflight_txs.remove(&tx_id);
            }
            self.wallet_indexing = !self.wallet_inflight_txs.is_empty();
        }
    }

    fn move_selection_down(&mut self) -> Option<usize> {
        if self.blocks.is_empty() {
            self.list_state.select(None);
            return None;
        }
        let len = self.blocks.len();
        let current = self.list_state.selected().unwrap_or(0);
        let new_idx = if current + 1 < len {
            current + 1
        } else {
            current
        };
        self.list_state.select(Some(new_idx));
        Some(new_idx)
    }

    fn move_selection_up(&mut self) -> Option<usize> {
        if self.blocks.is_empty() {
            self.list_state.select(None);
            return None;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new_idx = current.saturating_sub(1);
        self.list_state.select(Some(new_idx));
        Some(new_idx)
    }

    fn select_first_block(&mut self) -> Option<usize> {
        if self.blocks.is_empty() {
            self.list_state.select(None);
            None
        } else {
            self.list_state.select(Some(0));
            Some(0)
        }
    }

    fn select_last_block(&mut self) -> Option<usize> {
        if self.blocks.is_empty() {
            self.list_state.select(None);
            None
        } else {
            let idx = self.blocks.len() - 1;
            self.list_state.select(Some(idx));
            Some(idx)
        }
    }

    fn move_wallet_selection(&mut self, delta: i32) {
        if self.wallets.is_empty() {
            self.wallet_list_state.select(None);
            return;
        }
        let len = self.wallets.len() as i32;
        let current = self.wallet_list_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, len.saturating_sub(1)) as usize;
        self.wallet_list_state.select(Some(new_idx));
    }

    fn poll_wallet_worker(&mut self) {
        loop {
            match self.wallet_worker_rx.try_recv() {
                Ok(result) => self.handle_wallet_worker_result(result),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.wallet_index_message = Some("Wallet indexer stopped unexpectedly".into());
                    break;
                }
            }
        }
    }

    fn handle_wallet_worker_result(&mut self, result: WalletWorkerResult) {
        match result {
            WalletWorkerResult::ChunkComplete {
                tx_ids,
                deltas,
                range_start,
                range_end,
            } => {
                self.wallet_index_highest_synced = self.wallet_index_highest_synced.max(range_end);
                for tx_id in tx_ids {
                    self.wallet_inflight_txs.remove(&tx_id);
                    self.wallet_indexed_txs.insert(tx_id);
                }
                if !deltas.is_empty() {
                    self.apply_wallet_deltas(deltas);
                }
                self.wallet_indexing = !self.wallet_inflight_txs.is_empty();
                if self.wallet_indexing {
                    self.wallet_index_message = Some(format!(
                        "Indexed wallet chunk {}-{} ({} remaining, max height {})",
                        range_start,
                        range_end,
                        self.wallet_inflight_txs.len(),
                        self.wallet_index_highest_synced
                    ));
                } else {
                    self.wallet_index_message = Some(format!(
                        "Wallet index up to height {}",
                        self.wallet_index_highest_synced
                    ));
                }
                if self.wallet_indexing {
                    self.queue_wallet_index_work();
                }
            }
            WalletWorkerResult::Error { tx_ids, message } => {
                for tx_id in tx_ids {
                    self.wallet_inflight_txs.remove(&tx_id);
                }
                self.wallet_indexing = !self.wallet_inflight_txs.is_empty();
                self.wallet_index_message = Some(format!("Wallet index error: {}", message));
                self.queue_wallet_index_work();
            }
            WalletWorkerResult::Status(msg) => {
                self.wallet_index_message = Some(msg);
            }
        }
    }

    fn apply_wallet_deltas(&mut self, deltas: Vec<WalletDelta>) {
        for delta in deltas {
            let entry = self
                .wallet_map
                .entry(delta.address.clone())
                .or_insert_with(WalletTally::default);
            entry.total_received = entry.total_received.saturating_add(delta.received);
            entry.total_sent = entry.total_sent.saturating_add(delta.sent);
            entry.tx_count += delta.tx_count;
        }
        self.rebuild_wallets();
    }

    fn rebuild_wallets(&mut self) {
        let mut list: Vec<WalletSummary> = self
            .wallet_map
            .iter()
            .map(|(address, tally)| WalletSummary {
                address: address.clone(),
                total_received: tally.total_received,
                total_sent: tally.total_sent,
                tx_count: tally.tx_count,
            })
            .collect();
        list.sort_by(|a, b| self.compare_wallets(a, b));
        self.wallets = list;
        if self.wallets.is_empty() {
            self.wallet_list_state.select(None);
        } else {
            let current = self
                .wallet_list_state
                .selected()
                .unwrap_or(0)
                .min(self.wallets.len() - 1);
            self.wallet_list_state.select(Some(current));
        }
    }

    fn compare_wallets(&self, a: &WalletSummary, b: &WalletSummary) -> Ordering {
        let balance = |w: &WalletSummary| w.total_received.saturating_sub(w.total_sent);
        let primary = match self.wallet_sort_key {
            WalletSortKey::Balance => balance(a).cmp(&balance(b)),
            WalletSortKey::TotalReceived => a.total_received.cmp(&b.total_received),
            WalletSortKey::TotalSent => a.total_sent.cmp(&b.total_sent),
            WalletSortKey::TxCount => a.tx_count.cmp(&b.tx_count),
        };
        let primary = if self.wallet_sort_ascending {
            primary
        } else {
            primary.reverse()
        };
        if primary == Ordering::Equal {
            a.address.cmp(&b.address)
        } else {
            primary
        }
    }

    fn page_wallet_selection(&mut self, pages: i32) {
        let delta = pages.saturating_mul(PAGE_JUMP as i32);
        self.move_wallet_selection(delta);
    }

    fn select_first_wallet(&mut self) {
        if self.wallets.is_empty() {
            self.wallet_list_state.select(None);
        } else {
            self.wallet_list_state.select(Some(0));
        }
    }

    fn select_last_wallet(&mut self) {
        if self.wallets.is_empty() {
            self.wallet_list_state.select(None);
        } else {
            self.wallet_list_state.select(Some(self.wallets.len() - 1));
        }
    }

    fn set_wallet_sort_key(&mut self, key: WalletSortKey) {
        if self.wallet_sort_key != key {
            self.wallet_sort_key = key;
            self.wallet_sort_ascending = false;
            self.rebuild_wallets();
        }
    }

    fn toggle_wallet_sort_order(&mut self) {
        self.wallet_sort_ascending = !self.wallet_sort_ascending;
        self.rebuild_wallets();
    }

    fn cycle_tx_detail_focus(&mut self) {
        if let Some(state) = self.tx_detail.as_mut() {
            state.pane_focus = match state.pane_focus {
                TxDetailPane::Inputs => TxDetailPane::Outputs,
                TxDetailPane::Outputs => TxDetailPane::Inputs,
            };
        }
    }

    fn adjust_tx_pane_scroll(&mut self, delta: i32) {
        if let Some(state) = self.tx_detail.as_mut() {
            let scroll = match state.pane_focus {
                TxDetailPane::Inputs => &mut state.inputs_scroll,
                TxDetailPane::Outputs => &mut state.outputs_scroll,
            };
            if delta < 0 {
                let amount = (-delta).min(i32::from(u16::MAX)) as u16;
                *scroll = scroll.saturating_sub(amount);
            } else {
                let amount = (delta as u32).min(u16::MAX as u32) as u16;
                *scroll = scroll.saturating_add(amount);
            }
        }
    }

    fn page_tx_pane_scroll(&mut self, pages: i32) {
        let step = (PAGE_JUMP as i32) * pages;
        self.adjust_tx_pane_scroll(step);
    }

    fn home_tx_pane(&mut self) {
        if let Some(state) = self.tx_detail.as_mut() {
            match state.pane_focus {
                TxDetailPane::Inputs => state.inputs_scroll = 0,
                TxDetailPane::Outputs => state.outputs_scroll = 0,
            }
        }
    }

    fn end_tx_pane(&mut self) {
        if let Some(state) = self.tx_detail.as_mut() {
            match state.pane_focus {
                TxDetailPane::Inputs => state.inputs_scroll = u16::MAX,
                TxDetailPane::Outputs => state.outputs_scroll = u16::MAX,
            }
        }
    }

    fn move_transaction_list_selection(&mut self, delta: i32) -> Option<usize> {
        if self.transactions.is_empty() {
            self.tx_overview_state.select(None);
            return None;
        }
        let len = self.transactions.len();
        let current = self.tx_overview_state.selected().unwrap_or(0);
        let new_idx = (current as i32 + delta).clamp(0, (len - 1) as i32) as usize;
        self.tx_overview_state.select(Some(new_idx));
        Some(new_idx)
    }

    fn page_transaction_list_selection(&mut self, delta: i32) -> Option<usize> {
        let step = (PAGE_JUMP as i32) * delta;
        self.move_transaction_list_selection(step)
    }

    fn select_first_transaction(&mut self) -> Option<usize> {
        if self.transactions.is_empty() {
            self.tx_overview_state.select(None);
            None
        } else {
            self.tx_overview_state.select(Some(0));
            Some(0)
        }
    }

    fn select_last_transaction(&mut self) -> Option<usize> {
        if self.transactions.is_empty() {
            self.tx_overview_state.select(None);
            None
        } else {
            let idx = self.transactions.len() - 1;
            self.tx_overview_state.select(Some(idx));
            Some(idx)
        }
    }

    fn current_transaction_global_index(&self) -> Option<usize> {
        match self.view {
            View::TransactionsList => self.tx_overview_state.selected(),
            View::TransactionDetails { block_idx, tx_idx } => {
                self.transaction_index.get(&(block_idx, tx_idx)).copied()
            }
            View::BlockDetails(_) => {
                if let (Some(block_idx), Some(tx_idx)) =
                    (self.list_state.selected(), self.selected_tx_index())
                {
                    self.transaction_index.get(&(block_idx, tx_idx)).copied()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn set_transaction_overview_selection(&mut self, block_idx: usize, tx_idx: usize) {
        if let Some(idx) = self.transaction_index.get(&(block_idx, tx_idx)).copied() {
            self.tx_overview_state.select(Some(idx));
        }
    }

    async fn open_transaction_from_global_index(&mut self, idx: usize) -> Result<()> {
        if let Some(summary) = self.transactions.get(idx).cloned() {
            self.list_state.select(Some(summary.block_idx));
            self.block_focus = BlockDetailsFocus::Transactions;
            self.sync_tx_list_selection();
            if let Some(block) = self.blocks.get(summary.block_idx) {
                if summary.tx_idx < block.tx_ids.len() {
                    self.tx_list_state.select(Some(summary.tx_idx));
                }
            }
            self.tx_overview_state.select(Some(idx));
            self.open_transaction_detail(summary.block_idx, summary.tx_idx)
                .await?;
        }
        Ok(())
    }

    async fn navigate_transaction_delta(&mut self, delta: i32) -> Result<()> {
        if self.transactions.is_empty() {
            return Ok(());
        }
        if let Some(current) = self.current_transaction_global_index() {
            let len = self.transactions.len();
            let new_idx = (current as i32 + delta).clamp(0, (len - 1) as i32) as usize;
            self.open_transaction_from_global_index(new_idx).await?;
        }
        Ok(())
    }

    async fn sync_all_blocks(&mut self) -> Result<()> {
        while self.has_more_pages {
            self.load_next_page().await?;
        }
        self.status_message = Some("Synced all available pages".into());
        self.clear_status_on_input = true;
        Ok(())
    }

    fn scroll_help(&mut self, delta: i32) {
        let max = self.help_max_scroll;
        let new = if delta < 0 {
            self.help_scroll
                .saturating_sub(delta.unsigned_abs().min(u16::MAX as u32) as u16)
        } else {
            let inc = (delta as u32).min(u16::MAX as u32) as u16;
            self.help_scroll.saturating_add(inc)
        };
        self.help_scroll = new.min(max);
    }

    fn reset_help_scroll(&mut self) {
        self.help_scroll = 0;
    }

    fn end_help_scroll(&mut self) {
        self.help_scroll = self.help_max_scroll;
    }

    fn sync_tx_list_selection(&mut self) {
        self.tx_list_state = ListState::default();
        if let View::BlockDetails(idx) = self.view {
            if let Some(block) = self.blocks.get(idx) {
                if !block.tx_ids.is_empty() {
                    self.tx_list_state.select(Some(0));
                }
            }
        }
    }

    fn is_at_bottom(&self) -> bool {
        match self.list_state.selected() {
            Some(idx) if !self.blocks.is_empty() => idx == self.blocks.len() - 1,
            _ => false,
        }
    }

    fn should_auto_fetch_more(&self) -> bool {
        self.scroll_preload_enabled && self.has_more_pages && !self.loading && self.is_at_bottom()
    }

    fn page_down(&mut self) -> Option<usize> {
        if self.blocks.is_empty() {
            self.list_state.select(None);
            return None;
        }
        let len = self.blocks.len();
        let current = self.list_state.selected().unwrap_or(0);
        let jump = PAGE_JUMP.min(len.saturating_sub(1));
        let new_idx = (current + jump).min(len - 1);
        self.list_state.select(Some(new_idx));
        Some(new_idx)
    }

    fn page_up(&mut self) -> Option<usize> {
        if self.blocks.is_empty() {
            self.list_state.select(None);
            return None;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let jump = PAGE_JUMP.min(current);
        let new_idx = current.saturating_sub(jump);
        self.list_state.select(Some(new_idx));
        Some(new_idx)
    }

    fn current_block(&self) -> Option<&BlockEntry> {
        self.list_state
            .selected()
            .and_then(|idx| self.blocks.get(idx))
    }

    fn selected_tx_index(&self) -> Option<usize> {
        self.tx_list_state.selected()
    }

    fn selected_tx_id(&self) -> Option<String> {
        match (self.current_block(), self.selected_tx_index()) {
            (Some(block), Some(idx)) => block.tx_ids.get(idx).map(|tx| tx.hash.clone()),
            _ => None,
        }
    }

    fn select_first_tx(&mut self) {
        if let Some(block) = self.current_block() {
            if block.tx_ids.is_empty() {
                self.tx_list_state.select(None);
            } else {
                self.tx_list_state.select(Some(0));
            }
        } else {
            self.tx_list_state.select(None);
        }
    }

    fn select_last_tx(&mut self) {
        if let Some(block) = self.current_block() {
            if block.tx_ids.is_empty() {
                self.tx_list_state.select(None);
            } else {
                self.tx_list_state.select(Some(block.tx_ids.len() - 1));
            }
        } else {
            self.tx_list_state.select(None);
        }
    }

    fn copy_tx_id(&mut self, tx_id: &str) {
        self.copy_text_to_clipboard(tx_id, "Copied transaction id to clipboard");
    }

    fn copy_selected_block_id(&mut self) {
        let block = match self.current_block() {
            Some(block) => block,
            None => {
                self.error_message = Some("No block selected".into());
                self.status_message = None;
                self.clear_status_on_input = false;
                return;
            }
        };

        match hash_option_to_base58(&block.block_id) {
            Some(id) => self.copy_text_to_clipboard(&id, "Copied block id to clipboard"),
            None => {
                self.error_message = Some("Block id unavailable".into());
                self.status_message = None;
                self.clear_status_on_input = false;
            }
        }
    }

    fn copy_text_to_clipboard(&mut self, value: &str, success_message: &str) {
        self.error_message = None;
        match self.clipboard.as_mut() {
            Some(clip) => {
                if let Err(e) = clip.set_text(value.to_string()) {
                    self.error_message = Some(format!("Failed to copy: {}", e));
                    self.status_message = None;
                    self.clear_status_on_input = false;
                } else {
                    self.status_message = Some(success_message.into());
                    self.clear_status_on_input = true;
                    self.error_message = None;
                }
            }
            None => match Clipboard::new() {
                Ok(mut clip) => {
                    if let Err(e) = clip.set_text(value.to_string()) {
                        self.error_message = Some(format!("Failed to copy: {}", e));
                        self.status_message = None;
                        self.clear_status_on_input = false;
                    } else {
                        self.status_message = Some(success_message.into());
                        self.clear_status_on_input = true;
                        self.error_message = None;
                        self.clipboard = Some(clip);
                    }
                }
                Err(e) => {
                    self.error_message = Some(format!("Clipboard unavailable: {}", e));
                    self.status_message = None;
                    self.clear_status_on_input = false;
                }
            },
        }
    }

    fn read_clipboard_text(&mut self) -> Option<String> {
        match self.clipboard.as_mut() {
            Some(clip) => clip.get_text().ok(),
            None => match Clipboard::new() {
                Ok(mut clip) => match clip.get_text() {
                    Ok(text) => {
                        self.clipboard = Some(clip);
                        Some(text)
                    }
                    Err(_) => None,
                },
                Err(_) => None,
            },
        }
    }

    fn clear_status_if_needed(&mut self) {
        if self.clear_status_on_input {
            self.status_message = None;
            self.clear_status_on_input = false;
        }
    }

    fn move_tx_selection(&mut self, delta: i32) {
        let len = match self.current_block() {
            Some(block) => block.tx_ids.len(),
            None => return,
        };
        if len == 0 {
            self.tx_list_state.select(None);
            return;
        }
        let current = self.tx_list_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, (len - 1) as i32) as usize;
        self.tx_list_state.select(Some(new_idx));
    }

    fn note_user_action(&mut self) {
        self.last_user_action = Instant::now();
    }

    fn start_busy(&mut self) {
        self.busy = true;
        self.spinner_index = 0;
    }

    fn stop_busy(&mut self) {
        self.busy = false;
    }

    fn is_busy(&self) -> bool {
        self.busy
    }

    fn request_shutdown(&self) {
        self.shutdown_flag.store(true, AtomicOrdering::Release);
    }

    fn record_refresh(&mut self) {
        let now = Instant::now();
        self.last_refresh = now;
        self.next_allowed_refresh = now;
    }

    fn defer_auto_refresh(&mut self, delay: Duration) {
        self.next_allowed_refresh = Instant::now() + delay;
    }

    fn open_help(&mut self) {
        if !matches!(self.view, View::Help) {
            self.previous_view = Some(self.view.clone());
            self.set_view(View::Help);
            self.help_scroll = 0;
        }
    }

    fn close_help(&mut self) {
        if let Some(prev) = self.previous_view.take() {
            self.set_view(prev);
        } else {
            self.set_view(View::BlocksList);
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(3), // Tabs
            Constraint::Min(0),    // Main content
            Constraint::Length(3), // Status bar
            Constraint::Length(2), // Help
        ])
        .split(f.area());

    render_header(f, chunks[0], app);
    render_tabs(f, chunks[1], app);

    match &app.view {
        View::BlocksList => render_blocks_list(f, chunks[2], app),
        View::TransactionsList => render_transactions_list(f, chunks[2], app),
        View::WalletsList => render_wallets_list(f, chunks[2], app),
        View::Metrics => render_metrics_view(f, chunks[2], app),
        View::BlockDetails(idx) => render_block_details(f, chunks[2], app, *idx),
        View::TransactionDetails { block_idx, tx_idx } => {
            render_transaction_details(f, chunks[2], app, *block_idx, *tx_idx)
        }
        View::TransactionSearch => render_transaction_search(f, chunks[2], app),
        View::Help => render_help_menu(f, chunks[2], app),
    }

    render_status_bar(f, chunks[3], app);
    render_help(f, chunks[4], app);
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let connection_indicator = match app.connection_status {
        ConnectionStatus::Connected => {
            let age = app
                .last_successful_connection
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0);
            Span::styled(
                format!("● Connected ({}s ago)", age),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
        }
        ConnectionStatus::Disconnected => Span::styled(
            "● Disconnected",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        ConnectionStatus::Reconnecting => Span::styled(
            "● Reconnecting...",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        ConnectionStatus::NeverConnected => Span::styled(
            "● Never Connected",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    };

    let title = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                "Nockchain Block Explorer",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" | "),
            connection_indicator,
            Span::raw(" | "),
            Span::styled(
                format!("Height: {}", app.current_height),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::raw("Server: "),
            Span::styled(&app.server_uri, Style::default().fg(Color::Green)),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).title("Info"));
    f.render_widget(title, area);
}

fn render_tabs(f: &mut Frame, area: Rect, app: &App) {
    let titles = ["Blocks", "Transactions", "Wallets", "Metrics"]
        .iter()
        .map(|title| Line::from(Span::styled(*title, Style::default().fg(Color::Cyan))))
        .collect::<Vec<_>>();
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title("Views"))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .select(app.active_tab);
    f.render_widget(tabs, area);
}

fn render_blocks_list(f: &mut Frame, area: Rect, app: &mut App) {
    let items: Vec<ListItem> = app
        .blocks
        .iter()
        .map(|block| {
            let tx_count = block.tx_ids.len();
            let content = vec![Line::from(vec![
                Span::styled(
                    format!("Height: {:6}", block.height),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" | "),
                Span::styled(
                    format!("TXs: {:3}", tx_count),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" | "),
                Span::styled(
                    format!("Block ID: {}", hash_display(&block.block_id)),
                    Style::default().fg(Color::Green),
                ),
            ])];
            ListItem::new(content)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Blocks ({})", app.blocks.len())),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_transactions_list(f: &mut Frame, area: Rect, app: &mut App) {
    let items: Vec<ListItem> = app
        .transactions
        .iter()
        .map(|entry| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("Height: {:6}", entry.block_height),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" | "),
                Span::styled(
                    format!("Tx: {}", entry.tx_id),
                    Style::default().fg(Color::Green),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Transactions ({})", app.transactions.len())),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(list, area, &mut app.tx_overview_state);
}

fn render_wallets_list(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let status_message = app.wallet_index_message.clone().unwrap_or_else(|| {
        if app.wallet_index_highest_synced > 0 {
            format!(
                "Wallet index up to height {}",
                app.wallet_index_highest_synced
            )
        } else {
            "Wallet indexer idle".to_string()
        }
    });
    let sort_indicator = format!(
        "Sorting by {} {}",
        app.wallet_sort_key.label(),
        if app.wallet_sort_ascending {
            '↑'
        } else {
            '↓'
        }
    );
    let status = Paragraph::new(format!("{} | {}", status_message, sort_indicator))
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::ALL).title("Wallet Index"));
    f.render_widget(status, chunks[0]);

    if app.wallets.is_empty() {
        let empty = Paragraph::new("No wallet data indexed yet")
            .block(Block::default().borders(Borders::ALL).title("Wallets"));
        f.render_widget(empty, chunks[1]);
        return;
    }

    let items: Vec<ListItem> = app
        .wallets
        .iter()
        .map(|wallet| {
            let balance = wallet.total_received.saturating_sub(wallet.total_sent);
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:>6} tx | ", wallet.tx_count),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("{:<20}", short_hash_str(&wallet.address)),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" | Balance "),
                Span::styled(
                    format_wallet_nocks(balance),
                    Style::default().fg(Color::Magenta),
                ),
                Span::raw(" | Recv "),
                Span::styled(
                    format_wallet_nocks(wallet.total_received),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" | Sent "),
                Span::styled(
                    format_wallet_nocks(wallet.total_sent),
                    Style::default().fg(Color::LightMagenta),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(format!(
            "Wallets ({}) – {} {}",
            app.wallets.len(),
            app.wallet_sort_key.label(),
            if app.wallet_sort_ascending {
                '↑'
            } else {
                '↓'
            }
        )))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, chunks[1], &mut app.wallet_list_state);
}

fn render_metrics_view(f: &mut Frame, area: Rect, app: &mut App) {
    let mut lines = Vec::new();
    if let Some(metrics) = &app.metrics_data {
        lines.push(Line::from(vec![
            Span::styled(
                "Cache height: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(metrics.cache_height.to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Cache lowest height: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(metrics.cache_lowest_height.to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Cache span: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(metrics.cache_span.to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Coverage ratio: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{:.3}", metrics.cache_coverage_ratio)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Heaviest height: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(metrics.heaviest_height.to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Seed ready: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(if metrics.seed_ready { "yes" } else { "no" }),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Backfill resume height: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(if metrics.backfill_resume_height >= 0 {
                metrics.backfill_resume_height.to_string()
            } else {
                "none".to_string()
            }),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Cache age (s): ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{:.3}", metrics.cache_age_seconds)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Last refresh age (s): ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{:.3}", metrics.refresh_age_seconds)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Last backfill age (s): ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(if metrics.backfill_age_seconds >= 0.0 {
                format!("{:.3}", metrics.backfill_age_seconds)
            } else {
                "n/a".to_string()
            }),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Seed time (s): ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{:.3}", metrics.seed_time_seconds)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Refresh counts (ok/err): ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{}/{}",
                metrics.refresh_success_count, metrics.refresh_error_count
            )),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Backfill counts (ok/err): ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{}/{}",
                metrics.backfill_success_count, metrics.backfill_error_count
            )),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "GetBlocks latency p50/p90/p99 (ms): ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{:.3}/{:.3}/{:.3}",
                metrics.get_blocks_p50_ms, metrics.get_blocks_p90_ms, metrics.get_blocks_p99_ms
            )),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "BlockDetails latency p50/p90/p99 (ms): ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{:.3}/{:.3}/{:.3}",
                metrics.get_block_details_p50_ms,
                metrics.get_block_details_p90_ms,
                metrics.get_block_details_p99_ms
            )),
        ]));
    } else if let Some(err) = &app.metrics_error {
        lines.push(Line::from(Span::styled(
            format!("Metrics unavailable: {}", err),
            Style::default().fg(Color::Red),
        )));
    } else {
        lines.push(Line::from("Metrics not loaded yet"));
    }

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Explorer Metrics")
            .border_style(Style::default().fg(Color::Cyan)),
    );
    f.render_widget(paragraph, area);
}

fn render_block_details(f: &mut Frame, area: Rect, app: &mut App, idx: usize) {
    let block = match app.blocks.get(idx).cloned() {
        Some(b) => b,
        None => {
            let text = Paragraph::new("Block not found").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Block Details"),
            );
            f.render_widget(text, area);
            return;
        }
    };

    let full_details = app.full_block_details.get(&block.height).cloned();
    let loading_details = app.loading_block_details == Some(block.height);
    let block_focus = app.block_focus;

    // Split into left (block info) and right (transactions) panes
    // Block details get 75%, transactions get 25%
    let main_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(area);

    // Left pane: Block details
    render_block_info_pane(
        f,
        main_layout[0],
        &block,
        full_details.as_ref(),
        loading_details,
        block_focus,
    );

    // Right pane: Transactions list
    render_block_transactions_pane(f, main_layout[1], &block, app);
}

fn render_block_info_pane(
    f: &mut Frame,
    area: Rect,
    block: &BlockEntry,
    full_details: Option<&BlockDetails>,
    loading: bool,
    block_focus: BlockDetailsFocus,
) {
    let label_style = Style::default()
        .add_modifier(Modifier::BOLD)
        .fg(Color::Cyan);
    let value_style = Style::default().fg(Color::White);
    let hash_style = Style::default().fg(Color::Green);
    let dim_style = Style::default().fg(Color::DarkGray);
    let accent_style = Style::default().fg(Color::Yellow);

    let mut lines = Vec::new();

    // Header section
    lines.push(Line::from(vec![Span::styled(
        "═══ IDENTITY ═══",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(""));

    // Height and Version
    let version_str = if let Some(details) = full_details {
        format!("v{}", details.version)
    } else {
        "".to_string()
    };
    lines.push(Line::from(vec![
        Span::styled("  Height      ", label_style),
        Span::styled(
            format!("{}", block.height),
            accent_style.add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            version_str,
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Block ID
    lines.push(Line::from(vec![Span::styled(
        "  Block ID    ", label_style,
    )]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(hash_full_display(&block.block_id), hash_style),
    ]));

    // Parent ID
    lines.push(Line::from(vec![Span::styled(
        "  Parent      ", label_style,
    )]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(hash_full_display(&block.parent), dim_style),
    ]));

    lines.push(Line::from(""));

    // Consensus section
    lines.push(Line::from(vec![Span::styled(
        "═══ CONSENSUS ═══",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(""));

    // Timestamp
    lines.push(Line::from(vec![
        Span::styled("  Timestamp   ", label_style),
        Span::styled(format_timestamp(block.timestamp), value_style),
    ]));

    if let Some(details) = full_details {
        // Epoch Counter
        lines.push(Line::from(vec![
            Span::styled("  Epoch       ", label_style),
            Span::styled(format!("{}", details.epoch_counter), value_style),
        ]));

        // Proof of Work
        let pow_display = if details.has_pow {
            "✓ Present"
        } else {
            "✗ Missing"
        };
        let pow_color = if details.has_pow {
            Color::Green
        } else {
            Color::Red
        };
        lines.push(Line::from(vec![
            Span::styled("  PoW         ", label_style),
            Span::styled(pow_display, Style::default().fg(pow_color)),
        ]));

        // Target
        if let Some(ref target) = details.target {
            lines.push(Line::from(vec![Span::styled(
                "  Target      ", label_style,
            )]));
            let target_display = truncate_bignum_display(&target.display, 40);
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(target_display, dim_style),
            ]));
        }

        // Accumulated Work
        if let Some(ref work) = details.accumulated_work {
            lines.push(Line::from(vec![Span::styled(
                "  Acc. Work   ", label_style,
            )]));
            let work_display = truncate_bignum_display(&work.display, 40);
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(work_display, dim_style),
            ]));
        }
    } else if loading {
        lines.push(Line::from(vec![
            Span::styled("  ", dim_style),
            Span::styled("Loading details...", Style::default().fg(Color::Yellow)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  ", dim_style),
            Span::styled("(press Enter to load full details)", dim_style),
        ]));
    }

    lines.push(Line::from(""));

    // Content section
    lines.push(Line::from(vec![Span::styled(
        "═══ CONTENT ═══",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(""));

    // Transaction count
    lines.push(Line::from(vec![
        Span::styled("  Tx Count    ", label_style),
        Span::styled(format!("{}", block.tx_ids.len()), accent_style),
    ]));

    // Coinbase section (if we have full details)
    if let Some(details) = full_details {
        if let Some(ref coinbase) = details.coinbase {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "  Coinbase Rewards:", label_style,
            )]));
            render_coinbase_lines(&mut lines, coinbase);
        }

        // Message (if present)
        if let Some(ref msg) = details.msg {
            if !msg.decoded.is_empty() || !msg.raw.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![Span::styled(
                    "  Message     ", label_style,
                )]));
                let msg_text = if !msg.decoded.is_empty() {
                    msg.decoded.clone()
                } else if !msg.raw.is_empty() {
                    format!("(raw {} bytes)", msg.raw.len())
                } else {
                    "(empty)".to_string()
                };
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(msg_text, dim_style),
                ]));
            }
        }
    }

    let is_focused = block_focus == BlockDetailsFocus::Block;
    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(Span::styled(
                    " Block Details ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_coinbase_lines(
    lines: &mut Vec<Line>,
    coinbase: &nockapp_grpc_proto::pb::public::v2::CoinbaseSplit,
) {
    use nockapp_grpc_proto::pb::public::v2::coinbase_split::Version;
    let dim_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::Green);

    match &coinbase.version {
        Some(Version::V0(v0)) => {
            if v0.entry_count > 0 {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(format!("{} recipient(s) ", v0.entry_count), value_style),
                    Span::styled(v0.note.clone(), dim_style),
                ]));
            } else if !v0.note.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(v0.note.clone(), dim_style),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled("(v0 legacy format)", dim_style),
                ]));
            }
        }
        Some(Version::V1(v1)) => {
            if v1.entries.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled("(no entries)", dim_style),
                ]));
            } else {
                for entry in &v1.entries {
                    let lock_hash = entry
                        .lock_hash
                        .as_ref()
                        .map(|h| truncate_str(&h.hash, 16))
                        .unwrap_or_else(|| "???".to_string());
                    let amount = entry
                        .amount
                        .as_ref()
                        .map(|a| format_nicks(a.value))
                        .unwrap_or_else(|| "0".to_string());
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(lock_hash, dim_style),
                        Span::raw(" → "),
                        Span::styled(amount, value_style),
                    ]));
                }
            }
        }
        None => {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("(unknown format)", dim_style),
            ]));
        }
    }
}

fn render_block_transactions_pane(f: &mut Frame, area: Rect, block: &BlockEntry, app: &mut App) {
    if block.tx_ids.is_empty() {
        let text = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No transactions in this block",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Transactions "),
        );
        f.render_widget(text, area);
        return;
    }

    let tx_items: Vec<ListItem> = block
        .tx_ids
        .iter()
        .enumerate()
        .map(|(i, tx)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {:3} ", i + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(truncate_str(&tx.hash, 44), Style::default().fg(Color::Cyan)),
            ]))
        })
        .collect();

    let is_focused = app.block_focus == BlockDetailsFocus::Transactions;
    let border_style = if is_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if is_focused {
        format!(
            " Transactions ({}) [Enter: details, c: copy] ",
            block.tx_ids.len()
        )
    } else {
        format!(" Transactions ({}) [Tab to focus] ", block.tx_ids.len())
    };

    let list = List::new(tx_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, area, &mut app.tx_list_state);
}

fn truncate_bignum_display(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    }
}

fn format_nicks(nicks: u64) -> String {
    let nock = nicks as f64 / NICKS_PER_NOCK as f64;
    format!("{} NOCK", format_nock_value(nock))
}

fn render_transaction_details(
    f: &mut Frame,
    area: Rect,
    app: &mut App,
    block_idx: usize,
    tx_idx: usize,
) {
    let tx_state = match app.tx_detail.as_mut() {
        Some(detail) => detail,
        None => {
            let paragraph = Paragraph::new("Transaction details not loaded")
                .block(Block::default().borders(Borders::ALL).title("Transaction"));
            f.render_widget(paragraph, area);
            return;
        }
    };

    let status_snapshot = tx_state.status.clone();

    match status_snapshot {
        TxDetailStatus::Confirmed(details) => {
            let header_lines = build_tx_header_lines(tx_state, &details, block_idx, tx_idx);
            let header_height = header_lines.len().saturating_add(2) as u16;
            let header_constraint =
                Constraint::Length(header_height.min(area.height.saturating_sub(6).max(6)));

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([header_constraint, Constraint::Min(0)])
                .split(area);

            let header = Paragraph::new(header_lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Transaction Details"),
                )
                .wrap(Wrap { trim: false });
            f.render_widget(header, layout[0]);

            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(layout[1]);

            render_tx_inputs_section(
                f,
                body[0],
                &details,
                tx_state,
                tx_state.pane_focus == TxDetailPane::Inputs,
            );
            render_tx_outputs_section(
                f,
                body[1],
                &details,
                tx_state,
                tx_state.pane_focus == TxDetailPane::Outputs,
            );
        }
        TxDetailStatus::Pending => render_tx_status_message(
            f,
            area,
            tx_state,
            block_idx,
            tx_idx,
            "Pending (not yet included in a block)",
            Color::Yellow,
        ),
        TxDetailStatus::NotFound => render_tx_status_message(
            f,
            area,
            tx_state,
            block_idx,
            tx_idx,
            "Transaction not found on chain or in mempool",
            Color::Red,
        ),
        TxDetailStatus::Error(err) => render_tx_status_message(
            f,
            area,
            tx_state,
            block_idx,
            tx_idx,
            &format!("Error: {}", err),
            Color::Red,
        ),
    }
}

fn build_tx_header_lines(
    tx_state: &TxDetailState,
    details: &RpcTransactionDetails,
    block_idx: usize,
    tx_idx: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            "Transaction ID: ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(tx_state.tx_id.clone()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("Confirmed in block {}", details.height),
            Style::default().fg(Color::Green),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "Block ID: ",
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(Span::styled(
        hash_full_display(&details.block_id),
        Style::default().fg(Color::Green),
    )));
    lines.push(Line::from(vec![Span::styled(
        "Parent: ",
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(Span::styled(
        hash_full_display(&details.parent),
        Style::default().fg(Color::Yellow),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Height: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(details.height.to_string()),
        Span::raw("  Timestamp: "),
        Span::raw(format_timestamp(details.timestamp)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Version: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(details.version.to_string()),
        Span::raw("  Size: "),
        Span::raw(format!("{} bytes", format_number(details.size_bytes))),
    ]));
    // Totals section - multi-line format
    lines.push(Line::from(Span::styled(
        "Totals:",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::raw("  in:        "),
        Span::raw(format_amount(details.total_input.as_ref())),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  total out: "),
        Span::raw(format_amount(get_total_output_nicks(
            &details.total_output_required,
        ))),
    ]));
    // Calculate net sent = total_out - fee
    let total_out_nicks = get_total_output_nicks(&details.total_output_required)
        .map(|n| n.value)
        .unwrap_or(0);
    let fee_nicks = get_fee_nicks(&details.fee_required)
        .map(|n| n.value)
        .unwrap_or(0);
    let net_sent_nicks = total_out_nicks.saturating_sub(fee_nicks);
    let net_sent = pb_common::Nicks {
        value: net_sent_nicks,
    };
    lines.push(Line::from(vec![
        Span::raw("  net sent:  "),
        Span::raw(format_amount(Some(&net_sent))),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  fee:       "),
        Span::raw(format_amount(get_fee_nicks(&details.fee_required))),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "List position: ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("block {} • tx {}", block_idx, tx_idx)),
    ]));
    lines.push(Line::from(Span::styled(
        "ESC: Back • TAB: Switch pane • c: Copy tx id",
        Style::default().fg(Color::DarkGray),
    )));
    lines
}

fn render_tx_inputs_section(
    f: &mut Frame,
    area: Rect,
    details: &RpcTransactionDetails,
    tx_state: &mut TxDetailState,
    active: bool,
) {
    let lines = build_tx_input_lines(details);
    let max_scroll = lines
        .len()
        .saturating_sub(area.height.saturating_sub(2) as usize);
    let max_scroll_u16 = max_scroll.min(u16::MAX as usize) as u16;
    tx_state.inputs_scroll = tx_state.inputs_scroll.min(max_scroll_u16);

    let mut block = Block::default().borders(Borders::ALL).title("Inputs");
    if active {
        block = block.border_style(Style::default().fg(Color::Cyan));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((tx_state.inputs_scroll, 0));
    f.render_widget(paragraph, area);
}

fn build_tx_input_lines(details: &RpcTransactionDetails) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if details.inputs.is_empty() {
        lines.push(Line::from("No inputs"));
    } else {
        for (idx, input) in details.inputs.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{:02}] ", idx + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{} ", format_amount(input.amount.as_ref())),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    input.note_name_b58.clone(),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
            let mut source_desc = format!("source: {}", short_hash_str(&input.source_tx_id));
            if input.coinbase {
                source_desc.push_str(" (coinbase)");
            }
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(source_desc, Style::default().fg(Color::DarkGray)),
            ]));
            lines.push(Line::from(""));
        }
    }
    lines
}

fn render_tx_outputs_section(
    f: &mut Frame,
    area: Rect,
    details: &RpcTransactionDetails,
    tx_state: &mut TxDetailState,
    active: bool,
) {
    let lines = build_tx_output_lines(details);
    let max_scroll = lines
        .len()
        .saturating_sub(area.height.saturating_sub(2) as usize);
    let max_scroll_u16 = max_scroll.min(u16::MAX as usize) as u16;
    tx_state.outputs_scroll = tx_state.outputs_scroll.min(max_scroll_u16);

    let mut block = Block::default().borders(Borders::ALL).title("Outputs");
    if active {
        block = block.border_style(Style::default().fg(Color::Cyan));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((tx_state.outputs_scroll, 0));
    f.render_widget(paragraph, area);
}

fn build_tx_output_lines(details: &RpcTransactionDetails) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if details.outputs.is_empty() {
        lines.push(Line::from("No outputs"));
    } else {
        for (idx, output) in details.outputs.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{:02}] ", idx + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!(
                        "{} ",
                        format_amount(get_output_amount_nicks(&output.amount_required))
                    ),
                    Style::default().fg(Color::LightGreen),
                ),
                Span::styled(
                    output.note_name_b58.clone(),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(
                    format!("lock: {}", output.lock_summary.clone()),
                    Style::default().fg(Color::Magenta),
                ),
            ]));
            lines.push(Line::from(""));
        }
    }
    lines
}

fn render_tx_status_message(
    f: &mut Frame,
    area: Rect,
    tx_state: &TxDetailState,
    block_idx: usize,
    tx_idx: usize,
    message: &str,
    color: Color,
) {
    let lines = vec![
        Line::from(vec![
            Span::styled(
                "Transaction ID: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(tx_state.tx_id.clone()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(message, Style::default().fg(color)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "List position: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("block {} • tx {}", block_idx, tx_idx)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "ESC: Back to block • c: Copy tx id",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Transaction Details"),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn format_amount(amount: Option<&pb_common::Nicks>) -> String {
    let value = amount.map(|n| n.value).unwrap_or(0);
    let nock = value as f64 / NICKS_PER_NOCK as f64;
    format!(
        "{} nicks ({})",
        format_number(value),
        format_nock_value(nock)
    )
}

/// Extract Nicks from total_output oneof wrapper
fn get_total_output_nicks(
    oneof: &Option<transaction_details::TotalOutputRequired>,
) -> Option<&pb_common::Nicks> {
    match oneof {
        Some(transaction_details::TotalOutputRequired::TotalOutput(nicks)) => Some(nicks),
        None => None,
    }
}

/// Extract Nicks from fee oneof wrapper
fn get_fee_nicks(oneof: &Option<transaction_details::FeeRequired>) -> Option<&pb_common::Nicks> {
    match oneof {
        Some(transaction_details::FeeRequired::Fee(nicks)) => Some(nicks),
        None => None,
    }
}

/// Extract Nicks from output amount oneof wrapper
fn get_output_amount_nicks(
    oneof: &Option<transaction_output::AmountRequired>,
) -> Option<&pb_common::Nicks> {
    match oneof {
        Some(transaction_output::AmountRequired::Amount(nicks)) => Some(nicks),
        None => None,
    }
}

fn format_wallet_nocks(value: u64) -> String {
    let nock = value as f64 / NICKS_PER_NOCK as f64;
    format!("{} nock", format_nock_value(nock))
}

fn format_nock_value(value: f64) -> String {
    let mut s = format!("{:.6}", value);
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

fn short_hash_str(text: &str) -> String {
    if text.len() <= 16 {
        text.to_string()
    } else {
        format!("{}...{}", &text[..8], &text[text.len() - 6..])
    }
}

fn format_number(value: u64) -> String {
    let s = value.to_string();
    let mut acc = String::with_capacity(s.len() + s.len() / 3);
    let mut count = 0;
    for ch in s.chars().rev() {
        if count > 0 && count % 3 == 0 {
            acc.push('_');
        }
        acc.push(ch);
        count += 1;
    }
    acc.chars().rev().collect()
}

fn render_transaction_search(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // Input box
    let input = Paragraph::new(app.tx_search_input.as_str())
        .style(Style::default().fg(Color::Yellow))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Transaction ID prefix (base58, Enter to search)"),
        );
    f.render_widget(input, chunks[0]);

    // Results
    let result_text = match &app.tx_search_result {
        Some(TxSearchResult::Found(block_data)) => {
            vec![
                Line::from(vec![
                    Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(
                        "CONFIRMED",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "Block Height: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(block_data.height.to_string()),
                ]),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Block ID: ",
                    Style::default().add_modifier(Modifier::BOLD),
                )]),
                Line::from(Span::styled(
                    hash_full_display(&block_data.block_id),
                    Style::default().fg(Color::Green),
                )),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Parent ID: ",
                    Style::default().add_modifier(Modifier::BOLD),
                )]),
                Line::from(Span::styled(
                    hash_full_display(&block_data.parent),
                    Style::default().fg(Color::Yellow),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Timestamp: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format_timestamp(block_data.timestamp)),
                ]),
            ]
        }
        Some(TxSearchResult::Pending) => {
            vec![
                Line::from(vec![
                    Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(
                        "PENDING",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
                Line::from("Transaction exists in mempool but not yet in a block."),
            ]
        }
        Some(TxSearchResult::NotFound) => {
            vec![
                Line::from(vec![
                    Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(
                        "NOT FOUND",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
                Line::from("Transaction does not exist in blockchain or mempool."),
            ]
        }
        Some(TxSearchResult::Error(err)) => {
            vec![
                Line::from(vec![Span::styled(
                    "Error: ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from(Span::raw(err)),
            ]
        }
        None => {
            vec![Line::from(Span::styled(
                "Enter a transaction ID prefix and press Enter (Ctrl+V or Shift+Insert pastes)",
                Style::default().fg(Color::DarkGray),
            ))]
        }
    };

    let results = Paragraph::new(result_text)
        .block(Block::default().borders(Borders::ALL).title("Results"))
        .wrap(Wrap { trim: false });
    f.render_widget(results, chunks[1]);
}

fn render_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let status_text = if let Some(err) = &app.error_message {
        vec![Line::from(vec![
            Span::styled(
                "Error: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(err, Style::default().fg(Color::Red)),
        ])]
    } else if !matches!(app.connection_status, ConnectionStatus::Connected) {
        // Show connection error when not connected
        let mut lines = vec![Line::from(vec![
            Span::styled(
                "Disconnected: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.last_connection_error
                    .as_deref()
                    .unwrap_or("Unknown error"),
                Style::default().fg(Color::Red),
            ),
        ])];

        if let Some(last_success) = app.last_successful_connection {
            lines.push(Line::from(vec![
                Span::raw("Last successful connection: "),
                Span::styled(
                    format_duration(last_success.elapsed()),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" ago"),
            ]));
        }

        lines
    } else if app.loading {
        vec![Line::from(Span::styled(
            "Loading...",
            Style::default().fg(Color::Yellow),
        ))]
    } else if let Some(msg) = &app.status_message {
        vec![Line::from(vec![
            Span::styled(
                "Status: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(msg, Style::default().fg(Color::Cyan)),
        ])]
    } else {
        let age = app.last_refresh.elapsed().as_secs();
        let refresh_status = format!("Last refresh: {}s ago (manual)", age);
        let mut spans = vec![
            Span::styled("Ready", Style::default().fg(Color::Green)),
            Span::raw(" | "),
            Span::raw(refresh_status),
        ];
        if app.has_more_pages {
            spans.push(Span::raw(" | "));
            spans.push(Span::styled(
                "More pages available",
                Style::default().fg(Color::Cyan),
            ));
        }
        if let Some(msg) = &app.wallet_index_message {
            spans.push(Span::raw(" | "));
            spans.push(Span::styled(
                format!("Wallets: {}", msg),
                Style::default().fg(Color::Magenta),
            ));
        }
        vec![Line::from(spans)]
    };

    let status =
        Paragraph::new(status_text).block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(status, area);

    if app.is_busy() {
        render_busy_overlay(f, app);
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn render_help(f: &mut Frame, area: Rect, app: &App) {
    let help_text = match &app.view {
        View::BlocksList => {
            "↑: Up | ↓: Down (auto-loads older blocks) | PgUp/PgDn: Jump | Enter: View block | c: Copy block id | t: Search TX | r: Refresh | n: Next Page | s: Sync all pages | Tab/Shift+Tab: Switch tabs | ?: Help | q: Quit"
        }
        View::TransactionsList => {
            "↑: Up | ↓: Down | PgUp/PgDn: Jump | Home/End: First/last | Enter: View tx | Esc: Back to blocks | Tab/Shift+Tab: Switch tabs | n/p: Next/Prev tx | s: Sync all pages | ?: Help | q: Quit"
        }
        View::WalletsList => {
            "↑: Up | ↓: Down | PgUp/PgDn: Jump | Home/End: First/last | b/r/e/t: Sort balance/recv/sent/tx | o: Toggle order | Tab/Shift+Tab: Switch tabs | s: Sync all pages | ?: Help | q: Quit"
        }
        View::BlockDetails(_) => {
            "ESC: Back | ↑↓/PgUp/PgDn: Navigate blocks | Tab: Toggle focus | Enter: TX details | c: Copy tx | n/p: Next/Prev tx | ?: Help | q: Quit"
        }
        View::TransactionDetails { .. } => "ESC: Back | Tab: Switch pane | ↑/↓/PgUp/PgDn/Home/End: Scroll pane | n/p: Next/Prev tx | c: Copy tx id | ?: Help | q: Quit",
        View::TransactionSearch => {
            "ESC: Back | Enter: Search (prefix ok) | Ctrl+V/Ctrl+Shift+V: Paste | Ctrl+C: Clear | ?: Help | q: Quit"
        }
        View::Metrics => {
            "ESC: Back | r: Refresh metrics | Tab/Shift+Tab: Switch tabs | ?: Help | q: Quit"
        }
        View::Help => "ESC/q/?: Close help",
    };

    let help = Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, area);
}

fn hash_display(hash: &Option<nockapp_grpc_proto::pb::common::v1::Hash>) -> String {
    hash.as_ref()
        .map(|h| {
            let full = hash_to_string(h);
            if full.len() > 16 {
                format!("{}...{}", &full[..8], &full[full.len() - 8..])
            } else {
                full
            }
        })
        .unwrap_or_else(|| "(none)".to_string())
}

fn hash_full_display(hash: &Option<nockapp_grpc_proto::pb::common::v1::Hash>) -> String {
    hash.as_ref()
        .map(hash_to_string)
        .unwrap_or_else(|| "(none)".to_string())
}

fn hash_option_to_base58(hash: &Option<pb_common::Hash>) -> Option<String> {
    hash.as_ref()
        .and_then(|h| proto_hash_to_tip5(h))
        .map(|h| h.to_base58())
}

fn proto_hash_to_tip5(hash: &pb_common::Hash) -> Option<Tip5Hash> {
    Some(Tip5Hash([
        Belt(hash.belt_1.as_ref()?.value),
        Belt(hash.belt_2.as_ref()?.value),
        Belt(hash.belt_3.as_ref()?.value),
        Belt(hash.belt_4.as_ref()?.value),
        Belt(hash.belt_5.as_ref()?.value),
    ]))
}

fn hash_to_string(hash: &nockapp_grpc_proto::pb::common::v1::Hash) -> String {
    format!(
        "{:016x}.{:016x}.{:016x}.{:016x}.{:016x}",
        hash.belt_1.as_ref().map(|b| b.value).unwrap_or(0),
        hash.belt_2.as_ref().map(|b| b.value).unwrap_or(0),
        hash.belt_3.as_ref().map(|b| b.value).unwrap_or(0),
        hash.belt_4.as_ref().map(|b| b.value).unwrap_or(0),
        hash.belt_5.as_ref().map(|b| b.value).unwrap_or(0),
    )
}

fn format_timestamp(raw_ts: u64) -> String {
    // Blocks encode timestamps using `time-in-secs` on an Urbit `@da`, which means we need to
    // subtract the Urbit base epoch (2^63 plus an offset) to get back to real Unix seconds.
    const BASE_URBIT_EPOCH: u64 = 0x8000_000c_ce9e_0d80;

    let unix_secs = match raw_ts.checked_sub(BASE_URBIT_EPOCH) {
        Some(secs) => secs as i64,
        None => return format!("{} (before Urbit epoch base)", raw_ts),
    };

    match Utc.timestamp_opt(unix_secs, 0).single() {
        Some(dt) => {
            let age = Utc::now().signed_duration_since(dt);
            format!(
                "{} UTC ({})",
                dt.format("%Y-%m-%d %H:%M:%S"),
                format_relative_duration(age)
            )
        }
        None => format!("{} (invalid timestamp)", raw_ts),
    }
}

fn format_relative_duration(duration: ChronoDuration) -> String {
    let secs = duration.num_seconds();
    if secs == 0 {
        return "just now".to_string();
    }

    let suffix = if secs >= 0 { "ago" } else { "from now" };
    let mut remaining = secs.abs();

    let mut parts = Vec::new();
    let days = remaining / 86_400;
    if days > 0 {
        parts.push(format!("{}d", days));
        remaining %= 86_400;
    }
    let hours = remaining / 3_600;
    if hours > 0 && parts.len() < 2 {
        parts.push(format!("{}h", hours));
        remaining %= 3_600;
    }
    let minutes = remaining / 60;
    if minutes > 0 && parts.len() < 2 {
        parts.push(format!("{}m", minutes));
        remaining %= 60;
    }
    if remaining > 0 && parts.len() < 2 {
        parts.push(format!("{}s", remaining));
    }

    if parts.is_empty() {
        format!("less than 1s {}", suffix)
    } else {
        format!("{} {}", parts.join(" "), suffix)
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        Ok(Self { terminal })
    }

    fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn render_busy_overlay(f: &mut Frame, app: &App) {
    if !app.is_busy() {
        return;
    }
    let area = f.area();
    let popup_area = centered_rect(50, 20, area);
    let frame = SPINNER_FRAMES[app.spinner_index % SPINNER_FRAMES.len()];
    let color = SPINNER_COLORS[app.spinner_index % SPINNER_COLORS.len()];
    let lines = vec![
        Line::from(vec![Span::styled(
            "Working…",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                frame,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Please wait while the request completes…"),
        ]),
    ];
    f.render_widget(Clear, popup_area);
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Loading")
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup_area,
    );
}

#[tracing::instrument(name = "tui.run_app", skip(terminal, app))]
async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: App,
) -> Result<()> {
    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui(f, &mut app))?;
        if app.shutdown_flag.load(AtomicOrdering::Acquire) {
            return Ok(());
        }
        app.poll_wallet_worker();

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    app.clear_status_if_needed();
                    app.note_user_action();
                    match &app.view {
                        View::BlocksList => match key.code {
                            KeyCode::Char('q') => {
                                app.request_shutdown();
                                return Ok(());
                            }
                            KeyCode::Down => {
                                app.move_selection_down();
                                if app.should_auto_fetch_more() {
                                    app.request_next_page = true;
                                }
                            }
                            KeyCode::Up => {
                                app.move_selection_up();
                            }
                            KeyCode::PageDown => {
                                app.page_down();
                                if app.should_auto_fetch_more() {
                                    app.request_next_page = true;
                                }
                            }
                            KeyCode::PageUp => {
                                app.page_up();
                            }
                            KeyCode::Home => {
                                app.select_first_block();
                            }
                            KeyCode::End => {
                                app.select_last_block();
                            }
                            KeyCode::Char('c') => {
                                app.copy_selected_block_id();
                            }
                            KeyCode::Char('?') => app.open_help(),
                            KeyCode::Enter => {
                                if let Some(idx) = app.list_state.selected() {
                                    app.set_view(View::BlockDetails(idx));
                                    app.block_focus = if app
                                        .blocks
                                        .get(idx)
                                        .map(|b| b.tx_ids.is_empty())
                                        .unwrap_or(true)
                                    {
                                        BlockDetailsFocus::Block
                                    } else {
                                        BlockDetailsFocus::Transactions
                                    };
                                    app.sync_tx_list_selection();
                                    // Load full block details
                                    if let Some(block) = app.blocks.get(idx) {
                                        let height = block.height;
                                        app.start_busy();
                                        terminal.draw(|f| ui(f, &mut app))?;
                                        let _ = app.load_full_block_details(height).await;
                                        app.stop_busy();
                                    }
                                }
                            }
                            KeyCode::Char('t') => {
                                app.set_view(View::TransactionSearch);
                                app.tx_search_input.clear();
                                app.tx_search_result = None;
                            }
                            KeyCode::Char('r') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.refresh().await;
                                app.stop_busy();
                                res?;
                            }
                            KeyCode::Char('n') => {
                                if app.has_more_pages {
                                    app.start_busy();
                                    terminal.draw(|f| ui(f, &mut app))?;
                                    let res = app.load_next_page().await;
                                    app.stop_busy();
                                    res?;
                                }
                            }
                            KeyCode::Char('s') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.sync_all_blocks().await;
                                app.stop_busy();
                                res?;
                            }
                            KeyCode::Tab => app.cycle_tabs(1),
                            KeyCode::BackTab => app.cycle_tabs(-1),
                            _ => {}
                        },
                        View::TransactionsList => match key.code {
                            KeyCode::Char('q') => {
                                app.request_shutdown();
                                return Ok(());
                            }
                            KeyCode::Esc => {
                                app.set_view(View::BlocksList);
                            }
                            KeyCode::Tab => app.cycle_tabs(1),
                            KeyCode::BackTab => app.cycle_tabs(-1),
                            KeyCode::Down => {
                                app.move_transaction_list_selection(1);
                            }
                            KeyCode::Up => {
                                app.move_transaction_list_selection(-1);
                            }
                            KeyCode::PageDown => {
                                app.page_transaction_list_selection(1);
                            }
                            KeyCode::PageUp => {
                                app.page_transaction_list_selection(-1);
                            }
                            KeyCode::Home => {
                                app.select_first_transaction();
                            }
                            KeyCode::End => {
                                app.select_last_transaction();
                            }
                            KeyCode::Enter => {
                                if let Some(idx) = app.tx_overview_state.selected() {
                                    app.start_busy();
                                    terminal.draw(|f| ui(f, &mut app))?;
                                    let res = app.open_transaction_from_global_index(idx).await;
                                    app.stop_busy();
                                    res?;
                                }
                            }
                            KeyCode::Char('n') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.navigate_transaction_delta(1).await;
                                app.stop_busy();
                                res?;
                            }
                            KeyCode::Char('p') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.navigate_transaction_delta(-1).await;
                                app.stop_busy();
                                res?;
                            }
                            KeyCode::Char('s') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.sync_all_blocks().await;
                                app.stop_busy();
                                res?;
                            }
                            KeyCode::Char('?') => app.open_help(),
                            _ => {}
                        },
                        View::WalletsList => match key.code {
                            KeyCode::Char('q') => {
                                app.request_shutdown();
                                return Ok(());
                            }
                            KeyCode::Esc => app.set_view(View::BlocksList),
                            KeyCode::Tab => app.cycle_tabs(1),
                            KeyCode::BackTab => app.cycle_tabs(-1),
                            KeyCode::Down => app.move_wallet_selection(1),
                            KeyCode::Up => app.move_wallet_selection(-1),
                            KeyCode::PageDown => app.page_wallet_selection(1),
                            KeyCode::PageUp => app.page_wallet_selection(-1),
                            KeyCode::Home => {
                                app.select_first_wallet();
                            }
                            KeyCode::End => {
                                app.select_last_wallet();
                            }
                            KeyCode::Char('b') => app.set_wallet_sort_key(WalletSortKey::Balance),
                            KeyCode::Char('r') => {
                                app.set_wallet_sort_key(WalletSortKey::TotalReceived)
                            }
                            KeyCode::Char('e') => app.set_wallet_sort_key(WalletSortKey::TotalSent),
                            KeyCode::Char('t') => app.set_wallet_sort_key(WalletSortKey::TxCount),
                            KeyCode::Char('o') => app.toggle_wallet_sort_order(),
                            KeyCode::Char('s') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.sync_all_blocks().await;
                                app.stop_busy();
                                res?;
                            }
                            KeyCode::Char('?') => app.open_help(),
                            _ => {}
                        },
                        View::Metrics => match key.code {
                            KeyCode::Char('q') => {
                                app.request_shutdown();
                                return Ok(());
                            }
                            KeyCode::Esc => app.set_view(View::BlocksList),
                            KeyCode::Tab => app.cycle_tabs(1),
                            KeyCode::BackTab => app.cycle_tabs(-1),
                            KeyCode::Char('r') => {
                                app.metrics_error = None;
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let _ = app.load_metrics().await;
                                app.stop_busy();
                            }
                            KeyCode::Char('?') => app.open_help(),
                            _ => {}
                        },
                        View::BlockDetails(_) => match key.code {
                            KeyCode::Char('q') => {
                                app.request_shutdown();
                                return Ok(());
                            }
                            KeyCode::Esc => {
                                app.set_view(View::BlocksList);
                                app.block_focus = BlockDetailsFocus::Block;
                            }
                            KeyCode::Tab => {
                                if matches!(app.block_focus, BlockDetailsFocus::Block)
                                    && app.selected_tx_id().is_some()
                                {
                                    app.block_focus = BlockDetailsFocus::Transactions;
                                } else {
                                    app.block_focus = BlockDetailsFocus::Block;
                                }
                            }
                            KeyCode::Home => {
                                if matches!(app.block_focus, BlockDetailsFocus::Transactions) {
                                    app.select_first_tx();
                                } else if let Some(idx) = app.select_first_block() {
                                    app.set_view(View::BlockDetails(idx));
                                    app.sync_tx_list_selection();
                                    if let Some(block) = app.blocks.get(idx) {
                                        let height = block.height;
                                        app.start_busy();
                                        terminal.draw(|f| ui(f, &mut app))?;
                                        let _ = app.load_full_block_details(height).await;
                                        app.stop_busy();
                                    }
                                }
                            }
                            KeyCode::End => {
                                if matches!(app.block_focus, BlockDetailsFocus::Transactions) {
                                    app.select_last_tx();
                                } else if let Some(idx) = app.select_last_block() {
                                    app.set_view(View::BlockDetails(idx));
                                    app.sync_tx_list_selection();
                                    if let Some(block) = app.blocks.get(idx) {
                                        let height = block.height;
                                        app.start_busy();
                                        terminal.draw(|f| ui(f, &mut app))?;
                                        let _ = app.load_full_block_details(height).await;
                                        app.stop_busy();
                                    }
                                }
                            }
                            KeyCode::Char('n') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.navigate_transaction_delta(1).await;
                                app.stop_busy();
                                res?;
                            }
                            KeyCode::Char('p') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.navigate_transaction_delta(-1).await;
                                app.stop_busy();
                                res?;
                            }
                            KeyCode::Char('c') => {
                                if let Some(tx_id) = app.selected_tx_id() {
                                    app.copy_tx_id(&tx_id);
                                }
                            }
                            KeyCode::Char('?') => app.open_help(),
                            KeyCode::Enter => {
                                if matches!(app.block_focus, BlockDetailsFocus::Transactions) {
                                    if let (Some(block_idx), Some(tx_idx)) =
                                        (app.list_state.selected(), app.selected_tx_index())
                                    {
                                        app.start_busy();
                                        terminal.draw(|f| ui(f, &mut app))?;
                                        let res =
                                            app.open_transaction_detail(block_idx, tx_idx).await;
                                        app.stop_busy();
                                        res?;
                                    }
                                }
                            }
                            KeyCode::Down => {
                                if app.block_focus == BlockDetailsFocus::Transactions {
                                    app.move_tx_selection(1);
                                } else if let Some(idx) = app.move_selection_down() {
                                    app.set_view(View::BlockDetails(idx));
                                    app.sync_tx_list_selection();
                                    if let Some(block) = app.blocks.get(idx) {
                                        let height = block.height;
                                        // Queue fetch if not cached (non-blocking)
                                        if !app.full_block_details.contains_key(&height) {
                                            app.priority_prefetch_queue.push_front(height);
                                        }
                                        // Queue adjacent blocks for predictive prefetch
                                        app.queue_adjacent_prefetch(idx);
                                    }
                                    if app.should_auto_fetch_more() {
                                        app.request_next_page = true;
                                    }
                                }
                            }
                            KeyCode::Up => {
                                if app.block_focus == BlockDetailsFocus::Transactions {
                                    app.move_tx_selection(-1);
                                } else if let Some(idx) = app.move_selection_up() {
                                    app.set_view(View::BlockDetails(idx));
                                    app.sync_tx_list_selection();
                                    if let Some(block) = app.blocks.get(idx) {
                                        let height = block.height;
                                        // Queue fetch if not cached (non-blocking)
                                        if !app.full_block_details.contains_key(&height) {
                                            app.priority_prefetch_queue.push_front(height);
                                        }
                                        // Queue adjacent blocks for predictive prefetch
                                        app.queue_adjacent_prefetch(idx);
                                    }
                                }
                            }
                            KeyCode::PageDown => {
                                if let Some(idx) = app.page_down() {
                                    app.set_view(View::BlockDetails(idx));
                                    app.sync_tx_list_selection();
                                    if let Some(block) = app.blocks.get(idx) {
                                        let height = block.height;
                                        // Queue fetch if not cached (non-blocking)
                                        if !app.full_block_details.contains_key(&height) {
                                            app.priority_prefetch_queue.push_front(height);
                                        }
                                        // Queue adjacent blocks for predictive prefetch
                                        app.queue_adjacent_prefetch(idx);
                                    }
                                    if app.should_auto_fetch_more() {
                                        app.request_next_page = true;
                                    }
                                }
                            }
                            KeyCode::PageUp => {
                                if let Some(idx) = app.page_up() {
                                    app.set_view(View::BlockDetails(idx));
                                    app.sync_tx_list_selection();
                                    if let Some(block) = app.blocks.get(idx) {
                                        let height = block.height;
                                        // Queue fetch if not cached (non-blocking)
                                        if !app.full_block_details.contains_key(&height) {
                                            app.priority_prefetch_queue.push_front(height);
                                        }
                                        // Queue adjacent blocks for predictive prefetch
                                        app.queue_adjacent_prefetch(idx);
                                    }
                                }
                            }
                            _ => {}
                        },
                        View::TransactionDetails { block_idx, .. } => match key.code {
                            KeyCode::Char('q') => {
                                app.request_shutdown();
                                return Ok(());
                            }
                            KeyCode::Esc => {
                                app.set_view(View::BlockDetails(*block_idx));
                                app.block_focus = BlockDetailsFocus::Transactions;
                                app.sync_tx_list_selection();
                            }
                            KeyCode::Char('c') => {
                                if let Some(detail) = app.tx_detail.clone() {
                                    app.copy_tx_id(&detail.tx_id);
                                }
                            }
                            KeyCode::Char('?') => app.open_help(),
                            KeyCode::Tab => {
                                app.cycle_tx_detail_focus();
                            }
                            KeyCode::Down => {
                                app.adjust_tx_pane_scroll(1);
                            }
                            KeyCode::Up => {
                                app.adjust_tx_pane_scroll(-1);
                            }
                            KeyCode::PageDown => {
                                app.page_tx_pane_scroll(1);
                            }
                            KeyCode::PageUp => {
                                app.page_tx_pane_scroll(-1);
                            }
                            KeyCode::Home => {
                                app.home_tx_pane();
                            }
                            KeyCode::End => {
                                app.end_tx_pane();
                            }
                            KeyCode::Char('n') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.navigate_transaction_delta(1).await;
                                app.stop_busy();
                                res?;
                            }
                            KeyCode::Char('p') => {
                                app.start_busy();
                                terminal.draw(|f| ui(f, &mut app))?;
                                let res = app.navigate_transaction_delta(-1).await;
                                app.stop_busy();
                                res?;
                            }
                            _ => {}
                        },
                        View::TransactionSearch => match key.code {
                            KeyCode::Char('q') => {
                                app.request_shutdown();
                                return Ok(());
                            }
                            KeyCode::Esc => app.set_view(View::BlocksList),
                            KeyCode::Char('?') => app.open_help(),
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.tx_search_input.clear();
                                app.tx_search_result = None;
                            }
                            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                if let Some(text) = app.read_clipboard_text() {
                                    app.tx_search_input.push_str(&text);
                                }
                            }
                            KeyCode::Enter => {
                                if !app.tx_search_input.is_empty() {
                                    let search_input = app.tx_search_input.clone();
                                    app.start_busy();
                                    terminal.draw(|f| ui(f, &mut app))?;
                                    let res = app.search_transaction(&search_input).await;
                                    app.stop_busy();
                                    res?;
                                }
                            }
                            KeyCode::Backspace => {
                                app.tx_search_input.pop();
                            }
                            KeyCode::Char(c) => {
                                app.tx_search_input.push(c);
                            }
                            _ => {}
                        },
                        View::Help => match key.code {
                            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                                app.close_help()
                            }
                            KeyCode::Down => app.scroll_help(1),
                            KeyCode::Up => app.scroll_help(-1),
                            KeyCode::PageDown => app.scroll_help(PAGE_JUMP as i32),
                            KeyCode::PageUp => app.scroll_help(-(PAGE_JUMP as i32)),
                            KeyCode::Home => app.reset_help_scroll(),
                            KeyCode::End => app.end_help_scroll(),
                            _ => {}
                        },
                    }
                }
                Event::Paste(data) => {
                    app.clear_status_if_needed();
                    app.note_user_action();
                    if matches!(app.view, View::TransactionSearch) {
                        app.tx_search_input.push_str(&data);
                    }
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            // Check for pending user input - skip slow operations if user is interacting
            let has_pending_input = crossterm::event::poll(Duration::ZERO).unwrap_or(false);

            // Auto-reconnect if disconnected (skip if user has pending input)
            if !has_pending_input && app.should_retry_connection() {
                let _ = app.attempt_reconnect().await; // Don't fail on reconnect error
            }

            // Process priority prefetch first (user-navigated blocks)
            // Skip if user has pending input - prioritize UI responsiveness
            if !has_pending_input
                && !app.prefetch_in_progress
                && !app.priority_prefetch_queue.is_empty()
                && app.connection_status == ConnectionStatus::Connected
            {
                if let Some(height) = app.priority_prefetch_queue.pop_front() {
                    if !app.full_block_details.contains_key(&height) {
                        app.prefetch_in_progress = true;
                        let _ = app.load_full_block_details(height).await;
                        app.prefetch_in_progress = false;
                    }
                }
            }

            // Process background prefetch of block details (one at a time to avoid blocking UI)
            // Skip if user has pending input - prioritize UI responsiveness
            if !has_pending_input
                && !app.prefetch_in_progress
                && !app.prefetch_queue.is_empty()
                && app.connection_status == ConnectionStatus::Connected
            {
                if let Some(height) = app.prefetch_queue.pop_front() {
                    if !app.full_block_details.contains_key(&height) {
                        app.prefetch_in_progress = true;
                        let _ = app.load_full_block_details(height).await;
                        app.prefetch_in_progress = false;
                    }
                }
            }

            // Handle deferred page load request (from non-blocking navigation)
            // Skip if user has pending input - prioritize UI responsiveness
            if !has_pending_input
                && app.request_next_page
                && !app.prefetch_in_progress
                && app.connection_status == ConnectionStatus::Connected
            {
                app.request_next_page = false;
                let _ = app.load_next_page().await;
            }

            last_tick = Instant::now();
        }
    }
}

/// Check if CRASH_HAPPY env var is set. If so, panic with detailed error info.
/// This is useful for debugging deserialization errors during development.
fn crash_happy_check(context: &str, error: &impl std::fmt::Debug) {
    if env::var("CRASH_HAPPY").is_ok() {
        panic!(
            "\n\n=== CRASH_HAPPY TRIGGERED ===\n\
            Context: {}\n\
            Error: {:#?}\n\
            =============================\n",
            context, error
        );
    }
}

fn init_tracing() -> Result<()> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = fmt::layer().with_target(false);
    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer);
    let enable_tracy = env::var("TRACY_ENABLE").map(|v| v != "0").unwrap_or(false);

    if enable_tracy {
        registry
            .with(TracyLayer::default())
            .try_init()
            .map_err(|e| anyhow!(e.to_string()))?;
        info!("Tracing initialized with Tracy layer");
    } else {
        registry.try_init().map_err(|e| anyhow!(e.to_string()))?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing()?;
    // Parse CLI args
    let args = Args::parse();

    // Establish connection before touching the terminal so connection failures print normally.
    let app = App::new(args.server, args.fail_fast).await?;

    // Setup terminal with drop guard so panics/errors restore the TTY.
    let mut terminal = TerminalGuard::new()?;

    // Run app
    run_app(terminal.terminal(), app).await
}

#[tracing::instrument(
    name = "tui.wallet_index_worker",
    skip(command_rx, result_tx),
    fields(server = %server_uri)
)]
async fn wallet_index_worker(
    server_uri: String,
    mut command_rx: UnboundedReceiver<WalletWorkerCommand>,
    result_tx: UnboundedSender<WalletWorkerResult>,
) {
    let mut client: Option<NockchainBlockServiceClient<tonic::transport::Channel>> = None;
    while let Some(command) = command_rx.recv().await {
        let WalletWorkerCommand::IndexTransactions {
            tasks,
            range_start,
            range_end,
        } = command;
        if tasks.is_empty() {
            continue;
        }

        let mut completed = Vec::new();
        let mut delta_map: HashMap<String, WalletTally> = HashMap::new();
        for task in tasks {
            if client.is_none() {
                match NockchainBlockServiceClient::connect(server_uri.clone()).await {
                    Ok(new_client) => {
                        client = Some(new_client);
                        let _ = result_tx.send(WalletWorkerResult::Status(format!(
                            "Wallet indexer connected to {}",
                            server_uri
                        )));
                    }
                    Err(e) => {
                        let _ = result_tx.send(WalletWorkerResult::Error {
                            tx_ids: vec![task.tx_id],
                            message: format!("Wallet indexer connect error: {}", e),
                        });
                        continue;
                    }
                }
            }

            let request = GetTransactionDetailsRequest {
                tx_id: Some(Base58Hash {
                    hash: task.tx_id.clone(),
                }),
            };

            let fetch_result = client
                .as_mut()
                .expect("client should be connected")
                .get_transaction_details(Request::new(request))
                .await;

            match fetch_result {
                Ok(response) => match response.into_inner().result {
                    Some(get_transaction_details_response::Result::Details(details)) => {
                        accumulate_wallet_delta(&mut delta_map, &details);
                        completed.push(task.tx_id);
                    }
                    Some(get_transaction_details_response::Result::Pending(_)) => {
                        let _ = result_tx.send(WalletWorkerResult::Error {
                            tx_ids: vec![task.tx_id],
                            message: "Transaction pending confirmation".into(),
                        });
                    }
                    Some(get_transaction_details_response::Result::Error(err)) => {
                        let _ = result_tx.send(WalletWorkerResult::Error {
                            tx_ids: vec![task.tx_id],
                            message: err.message,
                        });
                    }
                    None => {
                        let _ = result_tx.send(WalletWorkerResult::Error {
                            tx_ids: vec![task.tx_id],
                            message: "Empty response from block service".into(),
                        });
                    }
                },
                Err(e) => {
                    let _ = result_tx.send(WalletWorkerResult::Error {
                        tx_ids: vec![task.tx_id],
                        message: format!("Wallet indexer RPC error: {}", e),
                    });
                    client = None;
                }
            }
        }

        if !completed.is_empty() {
            let deltas = delta_map
                .into_iter()
                .map(|(address, tally)| WalletDelta {
                    address,
                    received: tally.total_received,
                    sent: tally.total_sent,
                    tx_count: tally.tx_count,
                })
                .collect();
            let _ = result_tx.send(WalletWorkerResult::ChunkComplete {
                tx_ids: completed,
                deltas,
                range_start,
                range_end,
            });
        }
    }
}

fn accumulate_wallet_delta(
    map: &mut HashMap<String, WalletTally>,
    details: &RpcTransactionDetails,
) {
    let mut touched = HashSet::new();
    for input in &details.inputs {
        if let Some(address) = normalize_wallet_label(&input.note_name_b58) {
            let entry = map.entry(address.clone()).or_default();
            entry.total_sent = entry
                .total_sent
                .saturating_add(input.amount.as_ref().map(|n| n.value).unwrap_or(0));
            touched.insert(address);
        }
    }
    for output in &details.outputs {
        if let Some(address) = normalize_wallet_label(&output.note_name_b58) {
            let entry = map.entry(address.clone()).or_default();
            entry.total_received = entry.total_received.saturating_add(
                get_output_amount_nicks(&output.amount_required)
                    .map(|n| n.value)
                    .unwrap_or(0),
            );
            touched.insert(address);
        }
    }
    for address in touched {
        if let Some(entry) = map.get_mut(&address) {
            entry.tx_count += 1;
        }
    }
}

fn normalize_wallet_label(label: &str) -> Option<String> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn render_help_menu(f: &mut Frame, area: Rect, app: &mut App) {
    let sections = vec![
        (
            "Connection Status",
            vec![
                "● Connected       Server is reachable",
                "● Disconnected    Lost connection (will auto-retry every 5s)",
                "● Never Connected Still trying initial connection",
                "● Reconnecting    Attempting to reconnect now",
            ],
        ),
        (
            "Blocks List",
            vec![
                "↑          Move selection up",
                "↓          Move selection down (auto-loads older blocks)",
                "PgUp       Jump up by 20 blocks",
                "PgDn       Jump down by 20 blocks (auto-loads older)",
                "Enter      View selected block", "c          Copy selected block id",
                "Tab/Shift+Tab Switch between tabs", "t          Open transaction search",
                "r          Refresh newest page", "n          Fetch next page now",
                "s          Sync all pages", "?          Show this help", "q          Quit",
            ],
        ),
        (
            "Transactions List",
            vec![
                "ESC       Return to blocks", "Tab/Shift+Tab Switch tabs",
                "↑/↓        Move selection", "PgUp/PgDn Jump by 20 transactions",
                "Home/End  First/last transaction", "Enter     View transaction details",
                "n / p     Next/prev transaction detail", "s          Sync all pages",
                "?          Show this help",
            ],
        ),
        (
            "Wallets View",
            vec![
                "↑/↓        Move selection", "PgUp/PgDn Jump by 20 wallets",
                "Home/End  Jump to first/last wallet", "b/r/e/t   Sort balance/recv/sent/tx",
                "o          Toggle sort order", "Tab/Shift+Tab Switch tabs",
                "s          Sync all pages", "?          Show this help",
            ],
        ),
        (
            "Metrics",
            vec![
                "r          Refresh explorer metrics", "Tab/Shift+Tab Switch tabs",
                "ESC/q/?    Close help",
            ],
        ),
        (
            "Block Details",
            vec![
                "ESC        Back to list", "PgUp/PgDn  Jump to prev/next block",
                "Tab        Focus/unfocus transaction list", "↑/↓ (tx)   Move tx selection",
                "Enter      View selected transaction", "n / p      Next/prev transaction",
                "c          Copy highlighted tx id (tx focus)", "?          Show this help",
                "q          Quit",
            ],
        ),
        (
            "Transaction Search",
            vec![
                "ESC        Back to list", "Enter      Search for TX (prefix ok)",
                "Ctrl+V/Ctrl+Shift+V  Paste clipboard", "Ctrl+C     Clear input",
                "?          Show this help", "q          Quit",
            ],
        ),
        (
            "Transaction Details",
            vec![
                "ESC        Back", "Tab        Switch pane", "↑/↓/PgUp/PgDn Scroll inputs/outputs",
                "Home/End  Jump to start/end", "n / p      Next/prev transaction",
                "c          Copy transaction id", "?          Show this help", "q          Quit",
            ],
        ),
        (
            "Global",
            vec![
                "n / p     Next/prev transaction (details/list/block views)",
                "Tab/Shift+Tab  Switch top-level tabs",
            ],
        ),
    ];

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Keyboard Shortcuts",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (title, entries) in sections {
        lines.push(Line::from(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for entry in entries {
            lines.push(Line::from(Span::raw(format!("  {}", entry))));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        "Blocks you've already fetched stay cached locally; scrolling never re-downloads them.",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "Press ESC, q, or ? to close this help.",
        Style::default().fg(Color::DarkGray),
    )));

    let content_height = area.height.saturating_sub(2) as usize;
    let max_scroll = lines
        .len()
        .saturating_sub(content_height)
        .min(u16::MAX as usize) as u16;
    if app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }
    app.help_max_scroll = max_scroll;
    let has_above = app.help_scroll > 0;
    let has_below = app.help_scroll < max_scroll;
    let mut title = String::from("Help");
    if has_above {
        title.push('▲');
    }
    if has_below {
        title.push('▼');
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_alignment(Alignment::Center),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.help_scroll, 0));

    f.render_widget(paragraph, area);
}
