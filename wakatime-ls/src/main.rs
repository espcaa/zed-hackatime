use std::{collections::HashMap, fs, sync::Arc};

use arc_swap::ArcSwap;
use chrono::{DateTime, Local, TimeDelta};
use clap::{Arg, Command};
use serde::Deserialize;
use serde_json::Value;
use tokio::{process::Command as TokioCommand, sync::Mutex};
use tower_lsp::{jsonrpc::Result, lsp_types::*, Client, LanguageServer, LspService, Server};

#[derive(Deserialize, Default)]
struct Settings {
    api_key: Option<String>,
    api_url: Option<String>,
    metrics: Option<bool>,
    debug: Option<bool>,
    heartbeat_interval: Option<i64>,
}

#[derive(Debug, Clone)]
struct FileCacheEntry {
    lineno: u64,
    cursor_pos: u64,
}

#[derive(Debug, Default)]
struct FileCache {
    entries: HashMap<String, FileCacheEntry>,
}

type SharedFileCache = Arc<Mutex<FileCache>>;

#[derive(Default, Debug)]
struct Event {
    uri: String,
    is_write: bool,
    language: Option<String>,
    lineno: Option<u64>,
    cursor_pos: Option<u64>,
    file_changed: bool,
}

#[derive(Debug)]
struct CurrentFile {
    uri: String,
    timestamp: DateTime<Local>,
}

struct WakatimeLanguageServer {
    client: Client,
    settings: ArcSwap<Settings>,
    wakatime_path: String,
    current_file: Mutex<CurrentFile>,
    platform: ArcSwap<String>,
    file_cache: SharedFileCache,
}

// Extract filepath string from 'file://' URI.
//
// Example:
// file:///var/log/test.txt    -> /var/log/test.txt
// file:///C:/path/to/file.txt -> C:\path\to\file.txt
fn extract_uri_string(uri: &url::Url) -> String {
    uri.to_file_path()
        .map(|path: std::path::PathBuf| path.to_string_lossy().to_string())
        .unwrap_or_else(|()| uri[url::Position::BeforeUsername..].to_string())
}

impl WakatimeLanguageServer {
    async fn send(&self, event: Event) {
        if event.lineno.is_none() || event.cursor_pos.is_none() {
            // log message
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("Wakatime language server: no cursor position or line number info for file: {}, ignoring event", event.uri),
                )
                .await;
            return;
        }

        #[cfg(debug_assertions)]
        self.client
            .log_message(
                MessageType::LOG,
                format!("Wakatime language server send called, event: {event:?}",),
            )
            .await;

        // is_write -> send immediately ( don't update the timestamp for the interval check )
        // file_changed -> send immediately ( same )
        // else -> check interval, if now - last_sent > interval, send it and update timestamp

        let (last_timestamp, interval) = {
            let settings = self.settings.load();
            let interval = if let Some(heartbeat_interval) = settings.heartbeat_interval {
                TimeDelta::seconds(heartbeat_interval as i64)
            } else {
                TimeDelta::minutes(2)
            };

            let cf = self.current_file.lock().await;
            (cf.timestamp, interval)
        };

        let now = Local::now();

        #[cfg(debug_assertions)]
        self.client
            .log_message(
                MessageType::LOG,
                format!("Wakatime language server send called, event: {event:?}"),
            )
            .await;

        let should_send = event.is_write || event.file_changed || now - last_timestamp > interval;

        if should_send {
            #[cfg(debug_assertions)]
            self.client
                .log_message(
                    MessageType::LOG,
                    format!(
                        "Wakatime language server: sending heartbeat for file: {}, last sent at {}, interval reached",
                        event.uri, last_timestamp
                    ),
                )
                .await;
            let should_update_timestamp = !event.is_write && !event.file_changed;
            self.push_heartbeat(event, should_update_timestamp).await;
        } else {
            #[cfg(debug_assertions)]
            self.client
                .log_message(
                    MessageType::LOG,
                    format!(
                        "Wakatime language server: skipping heartbeat for file: {}, last sent at {}, interval not reached",
                        event.uri, last_timestamp
                    ),
                )
                .await;
            return;
        }
    }

    async fn push_heartbeat(&self, event: Event, update_timestamp: bool) {
        let now = Local::now();

        // get the line count of the file
        let line_count = fs::read_to_string(&event.uri)
            .map(|content| content.lines().count() as u64)
            .unwrap_or(0);

        let mut command = TokioCommand::new(self.wakatime_path.as_str());

        command
            .arg("--time")
            .arg((now.timestamp() as f64).to_string())
            .arg("--entity")
            .arg(event.uri.as_str());

        if !self.platform.load().is_empty() {
            command.arg("--plugin").arg(self.platform.load().as_str());
        }

        if event.is_write {
            command.arg("--write");
        }

        let settings = self.settings.load();

        if settings.metrics == Some(true) {
            command.arg("--metrics");
        }

        if let Some(ref key) = settings.api_key {
            command.arg("--key").arg(key);
        }

        if let Some(ref api_url) = settings.api_url {
            command.arg("--api-url").arg(api_url);
        }

        if let Some(ref language) = event.language {
            command.arg("--language").arg(language);
        } else {
            command.arg("--guess-language");
        }

        if let Some(ref debug) = settings.debug {
            if *debug {
                command.arg("--verbose");
            }
        }

        if let Some(lineno) = event.lineno {
            command.arg("--lineno").arg(lineno.to_string());
        }

        if let Some(cursor_pos) = event.cursor_pos {
            command.arg("--cursorpos").arg(cursor_pos.to_string());
        }

        if line_count > 0 {
            command.arg("--lines-in-file").arg(line_count.to_string());
        }

        self.client
            .log_message(
                MessageType::LOG,
                format!("Wakatime command: {:?}", command.as_std()),
            )
            .await;

        if let Err(e) = command.output().await {
            self.client
                .log_message(
                    MessageType::LOG,
                    format!(
                        "Wakatime language server send msg failed: {e:?}, command: {:?}",
                        command.as_std()
                    ),
                )
                .await;
        };

        if update_timestamp {
            let mut cf = self.current_file.lock().await;
            cf.timestamp = now;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for WakatimeLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(ref client_info) = params.client_info {
            let mut platform = String::new();
            platform.push_str("Zed");

            if let Some(ref version) = client_info.version {
                platform.push('/');
                platform.push_str(version.as_str());
            }

            platform.push(' ');
            platform.push_str(format!("Zed-hackatime/{}", env!("CARGO_PKG_VERSION")).as_str());

            self.platform.store(Arc::new(platform));
        }

        if let Some(initialization_options) = params.initialization_options {
            let initialization_options: Value = serde_json::from_value(initialization_options)
                .map_err(|_| "Could not parse settings (this should never happen)".to_string())
                .unwrap();

            let mut settings = Settings::default();

            // check if the plugin is disabled

            if let Some(api_url) = initialization_options
                .get("api-url")
                .and_then(Value::as_str)
            {
                settings.api_url = Some(api_url.to_string());
            }

            if let Some(api_key) = initialization_options
                .get("api-key")
                .and_then(Value::as_str)
            {
                settings.api_key = Some(api_key.to_string());
            }

            if let Some(metrics) = initialization_options
                .get("metrics")
                .and_then(Value::as_bool)
            {
                settings.metrics = Some(metrics);
            }

            if let Some(debug) = initialization_options.get("debug").and_then(Value::as_bool) {
                settings.debug = Some(debug);
            }

            self.settings.swap(Arc::from(settings));
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Wakatime language server initialized")
            .await;
        self.client
            .log_message(
                MessageType::INFO,
                "Hackatime version; only tracking events with line and cursor position will be sent.",
            )
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let file_uri = extract_uri_string(&params.text_document.uri);
        let file_changed = {
            let cf = self.current_file.lock().await;
            file_uri != cf.uri
        };

        let event = Event {
            uri: file_uri.clone(),
            is_write: false,
            lineno: params
                .content_changes
                .first()
                .and_then(|c| c.range)
                .map(|r| r.start.line as u64),
            language: None,
            cursor_pos: params
                .content_changes
                .first()
                .and_then(|c| c.range)
                .map(|r| r.start.character as u64),
            file_changed,
        };

        // add it to the cache

        {
            let mut cache = self.file_cache.lock().await;
            cache.entries.insert(
                file_uri.clone(),
                FileCacheEntry {
                    lineno: event.lineno.unwrap_or(0),
                    cursor_pos: event.cursor_pos.unwrap_or(0),
                },
            );
        }

        {
            let mut cf = self.current_file.lock().await;
            cf.uri = file_uri.clone();
        }

        self.send(event).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "did_change triggered")
            .await;

        // log it

        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "Wakatime language server: file saved: {}",
                    params.text_document.uri
                ),
            )
            .await;

        let file_uri = extract_uri_string(&params.text_document.uri);

        // check if the file is in the cache

        let cache = self.file_cache.lock().await;
        let (lineno, cursor_pos) = if let Some(entry) = cache.entries.get(&file_uri) {
            (Some(entry.lineno), Some(entry.cursor_pos))
        } else {
            (None, None)
        };

        if lineno.is_none() || cursor_pos.is_none() {
            // log message
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("Wakatime language server: no cursor position or line number info for saved file: {}, probably not in the cache, so we're ignoring it", file_uri),
                )
                .await;
            return;
        }

        let event = Event {
            uri: file_uri.clone(),
            is_write: true,
            lineno,
            language: None,
            cursor_pos,
            file_changed: false,
        };

        {
            let mut cf = self.current_file.lock().await;
            cf.uri = file_uri.clone();
        }

        self.send(event).await;
    }
}

#[tokio::main]
async fn main() {
    let matches = Command::new("wakatime_ls")
        .version(env!("CARGO_PKG_VERSION"))
        .author("bestgopher <84328409@qq.com>")
        .about("A simple WakaTime language server tool")
        .arg(
            Arg::new("wakatime-cli")
                .short('p')
                .long("wakatime-cli")
                .help("wakatime-cli path")
                .required(true),
        )
        .get_matches();

    let wakatime_cli = if let Some(s) = matches.get_one::<String>("wakatime-cli") {
        s.to_string()
    } else {
        "wakatime-cli".to_string()
    };

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| {
        Arc::new(WakatimeLanguageServer {
            client,
            settings: ArcSwap::from_pointee(Settings::default()),
            wakatime_path: wakatime_cli,
            platform: ArcSwap::from_pointee(String::new()),
            current_file: Mutex::new(CurrentFile {
                uri: String::new(),
                timestamp: Local::now(),
            }),
            file_cache: Arc::new(Mutex::new(FileCache::default())),
        })
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
