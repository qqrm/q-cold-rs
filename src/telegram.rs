use std::collections::BTreeSet;
use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{agents, history, repository, state, status, webapp};

#[derive(Args)]
pub struct TelegramArgs {
    #[command(subcommand)]
    command: TelegramCommand,
}

#[derive(Subcommand)]
enum TelegramCommand {
    #[command(about = "Poll Telegram updates and route operator commands")]
    Poll(PollArgs),
    #[command(about = "Serve the Telegram Mini App dashboard over HTTP")]
    Serve(webapp::ServeArgs),
}

#[derive(Args, Clone)]
struct PollArgs {
    #[arg(long, default_value_t = 20)]
    timeout: u64,
    #[arg(long)]
    once: bool,
}

pub fn run(args: TelegramArgs) -> Result<u8> {
    match args.command {
        TelegramCommand::Poll(args) => poll(&args)?,
        TelegramCommand::Serve(args) => webapp::serve(&args)?,
    }
    Ok(0)
}

fn poll(args: &PollArgs) -> Result<()> {
    let config = TelegramConfig::from_env()?;
    let client = TelegramClient::new(config.clone());
    if let Err(err) = client.set_my_commands() {
        eprintln!("Telegram setMyCommands failed: {err:#}");
    }
    let router = Router::new(config);
    let mut offset = None;

    loop {
        let updates = client.get_updates(offset, args.timeout)?;
        for update in updates {
            offset = Some(update.update_id + 1);
            if let Some(action) = router.route(&update)? {
                client.apply(action)?;
            }
        }
        if args.once {
            break;
        }
    }
    Ok(())
}

#[derive(Clone)]
struct TelegramConfig {
    api_base_url: String,
    bot_token: String,
    operator_chat_id: String,
    meta_chat_id: String,
    allowed_user_ids: BTreeSet<i64>,
    allowed_usernames: BTreeSet<String>,
    meta_agent_command: Option<String>,
    webapp_url: Option<String>,
    history_enabled: bool,
}

impl TelegramConfig {
    fn from_env() -> Result<Self> {
        let bot_token = required_env("TELEGRAM_BOT_TOKEN")?;
        let operator_chat_id = optional_env("QCOLD_TELEGRAM_OPERATOR_CHAT_ID")
            .or_else(|| optional_env("TELEGRAM_CHAT_ID"))
            .context("set QCOLD_TELEGRAM_OPERATOR_CHAT_ID or TELEGRAM_CHAT_ID")?;
        let meta_chat_id =
            optional_env("QCOLD_TELEGRAM_META_CHAT_ID").unwrap_or_else(|| operator_chat_id.clone());
        let allowed_user_ids =
            parse_allowed_users(optional_env("QCOLD_TELEGRAM_ALLOWED_USER_IDS").as_deref())?;
        let allowed_usernames =
            parse_allowed_usernames(optional_env("QCOLD_TELEGRAM_ALLOWED_USERNAMES").as_deref());
        if allowed_user_ids.is_empty() && allowed_usernames.is_empty() {
            bail!(
                "set QCOLD_TELEGRAM_ALLOWED_USER_IDS or QCOLD_TELEGRAM_ALLOWED_USERNAMES before enabling Telegram command execution"
            );
        }
        Ok(Self {
            api_base_url: optional_env("TELEGRAM_API_BASE_URL")
                .unwrap_or_else(|| "https://api.telegram.org".to_string()),
            bot_token,
            operator_chat_id,
            meta_chat_id,
            allowed_user_ids,
            allowed_usernames,
            meta_agent_command: optional_env("QCOLD_META_AGENT_COMMAND"),
            webapp_url: optional_env("QCOLD_TELEGRAM_WEBAPP_URL"),
            history_enabled: true,
        })
    }
}

fn required_env(name: &str) -> Result<String> {
    optional_env(name).with_context(|| format!("set {name}"))
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_allowed_users(value: Option<&str>) -> Result<BTreeSet<i64>> {
    let Some(value) = value else {
        return Ok(BTreeSet::new());
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            item.parse::<i64>()
                .with_context(|| format!("invalid Telegram user id: {item}"))
        })
        .collect()
}

fn parse_allowed_usernames(value: Option<&str>) -> BTreeSet<String> {
    let Some(value) = value else {
        return BTreeSet::new();
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.trim_start_matches('@').to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .collect()
}

struct Router {
    config: TelegramConfig,
}

impl Router {
    fn new(config: TelegramConfig) -> Self {
        Self { config }
    }

    fn route(&self, update: &TelegramUpdate) -> Result<Option<TelegramAction>> {
        let Some(message) = update.message.as_ref() else {
            return Ok(None);
        };
        if !self.allowed(message) {
            return Ok(None);
        }
        let Some(text) = message
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return Ok(None);
        };
        if self.config.history_enabled {
            if let Err(err) = history::append("telegram", "operator", text) {
                eprintln!("Telegram history append failed: {err:#}");
            }
        }

        if command_matches(text, "status") {
            return Ok(Some(TelegramAction::Send(
                message.reply(status::telegram_snapshot()?),
            )));
        }
        if command_matches(text, "agents") {
            return Ok(Some(TelegramAction::Send(
                message.reply(agents::snapshot()?),
            )));
        }
        if command_matches(text, "whoami") {
            return Ok(Some(TelegramAction::Send(message.reply(whoami_text(
                message,
                self.config.allowed_user_ids.is_empty() && self.config.allowed_usernames.is_empty(),
            )))));
        }
        if command_matches(text, "repos") || command_matches(text, "context") {
            return Ok(Some(TelegramAction::Send(
                message.reply(webapp::context_text()),
            )));
        }
        if command_matches(text, "app") || command_matches(text, "ui") {
            return Ok(Some(TelegramAction::Send(app_message(
                message,
                &self.config,
            ))));
        }
        if let Some(request) = command_payload(text, "task") {
            if !Self::is_task_creation_context(message)? {
                return Ok(Some(TelegramAction::Send(
                    message.reply(
                        "/task can only be created from the operator or meta chat general context."
                            .to_string(),
                    ),
                )));
            }
            let task_request = TaskRequest::new(message, request)?;
            return Ok(Some(TelegramAction::CreateTask(task_request)));
        }
        if let Some(request) = command_payload(text, "agent_start") {
            let response = match parse_agent_start(request) {
                Ok((track, command)) => match agents::start_shell_agent(track, command) {
                    Ok(record) => format!("Started agent:\n{}", agents::snapshot_line(&record)),
                    Err(err) => format!("Failed to start agent: {err:#}"),
                },
                Err(err) => err.to_string(),
            };
            return Ok(Some(TelegramAction::Send(message.reply(response))));
        }
        if command_matches(text, "help") || text == "/start" {
            return Ok(Some(TelegramAction::Send(message.reply(help_text()))));
        }
        if text.starts_with('/') {
            return Ok(Some(TelegramAction::Send(message.reply(
                "Unknown Q-COLD command. Try /status or /help.".to_string(),
            ))));
        }

        if let Some(thread_id) = message.message_thread_id {
            if let Some(task) = TaskState::load()?.find_by_thread(message.chat.id, thread_id) {
                append_task_event(&task.id, message, text)?;
                return Ok(Some(TelegramAction::Send(
                    message.reply(format!("Recorded input for task {}.", task.id)),
                )));
            }
        }

        if self.is_meta_chat(message)
            || Self::is_direct_operator_chat(message)
            || message.reply_to_message.is_some()
        {
            let response = meta_agent_reply(text, message, &self.config)?;
            return Ok(Some(TelegramAction::Send(message.reply(response))));
        }

        Ok(None)
    }

    fn allowed(&self, message: &TelegramMessage) -> bool {
        let Some(user) = message.from.as_ref() else {
            return false;
        };
        let id_allowed = self.config.allowed_user_ids.contains(&user.id);
        let username_allowed = user.username.as_ref().is_some_and(|username| {
            self.config
                .allowed_usernames
                .contains(&username.to_ascii_lowercase())
        });
        if !id_allowed && !username_allowed {
            return false;
        }
        self.is_operator_chat(message)
            || self.is_meta_chat(message)
            || Self::is_direct_operator_chat(message)
    }

    fn is_operator_chat(&self, message: &TelegramMessage) -> bool {
        message.chat.id.to_string() == self.config.operator_chat_id
    }

    fn is_meta_chat(&self, message: &TelegramMessage) -> bool {
        message.chat.id.to_string() == self.config.meta_chat_id
    }

    fn is_direct_operator_chat(message: &TelegramMessage) -> bool {
        message.chat.kind == "private"
            && message
                .from
                .as_ref()
                .is_some_and(|user| message.chat.id == user.id)
    }

    fn is_task_creation_context(message: &TelegramMessage) -> Result<bool> {
        let Some(thread_id) = message.message_thread_id else {
            return Ok(true);
        };
        Ok(TaskState::load()?
            .find_by_thread(message.chat.id, thread_id)
            .is_none())
    }
}

fn command_matches(text: &str, command: &str) -> bool {
    text == format!("/{command}") || text.starts_with(&format!("/{command}@"))
}

fn help_text() -> String {
    [
        "Q-COLD Telegram control plane",
        "/app - open the Q-COLD Mini App dashboard",
        "/repos - show connected repository context",
        "/whoami - show your Telegram user id",
        "/task <description> - create a task topic",
        "/status - show repository task state",
        "/agents - show Q-COLD managed agents",
        "/agent_start <track> :: <command> - start an agent through Q-COLD",
        "/help - show this help",
        "",
        "Plain messages in the meta chat and replies in allowed chats are routed to the meta-agent.",
        "Messages inside a task topic are recorded as task input.",
    ]
    .join("\n")
}

fn whoami_text(message: &TelegramMessage, unrestricted: bool) -> String {
    let user_id = message
        .from
        .as_ref()
        .map_or_else(|| "unknown".to_string(), |user| user.id.to_string());
    let username = message
        .from
        .as_ref()
        .and_then(|user| user.username.as_deref())
        .unwrap_or("unknown");
    let mode = if unrestricted {
        "Operator user allowlist is not configured yet."
    } else {
        "Operator user allowlist is active."
    };
    format!(
        "Telegram user id: {user_id}\nUsername: @{username}\nChat id: {}\n\n{mode}\nPrefer QCOLD_TELEGRAM_ALLOWED_USER_IDS={user_id} for a stable account lock.",
        message.chat.id
    )
}

fn app_message(message: &TelegramMessage, config: &TelegramConfig) -> SendMessage {
    let Some(url) = config.webapp_url.as_deref() else {
        return message.reply(
            "Q-COLD Mini App URL is not configured. Start `cargo qcold telegram serve --listen 127.0.0.1:8787 --daemon`, expose it through HTTPS, then set QCOLD_TELEGRAM_WEBAPP_URL."
                .to_string(),
        );
    };
    SendMessage {
        chat_id: message.chat.id.to_string(),
        message_thread_id: message.message_thread_id,
        text: "Open Q-COLD Mini App.".to_string(),
        reply_to_message_id: Some(message.message_id),
        reply_markup: Some(ReplyMarkup::WebAppButton {
            text: "Open Q-COLD".to_string(),
            url: url.to_string(),
        }),
    }
}

fn command_payload<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let (head, rest) = text.split_once(' ').unwrap_or((text, ""));
    if head == format!("/{command}") || head.starts_with(&format!("/{command}@")) {
        return Some(rest);
    }
    None
}

fn parse_agent_start(request: &str) -> Result<(&str, &str)> {
    let Some((track, command)) = request.split_once("::") else {
        anyhow::bail!("usage: /agent_start <track> :: <command>");
    };
    let track = track.trim();
    let command = command.trim();
    if track.is_empty() || command.is_empty() {
        anyhow::bail!("usage: /agent_start <track> :: <command>");
    }
    Ok((track, command))
}

fn meta_agent_reply(
    text: &str,
    message: &TelegramMessage,
    config: &TelegramConfig,
) -> Result<String> {
    let command = meta_agent_command(config.meta_agent_command.as_deref())?;
    let prompt = if config.history_enabled {
        history::prompt_context(text, 20)
            .unwrap_or_else(|_| format!("Current operator message:\n{}", text.trim()))
    } else {
        format!("Current operator message:\n{}", text.trim())
    };
    run_meta_agent_command(&command, &prompt, message)
}

fn run_meta_agent_command(command: &str, text: &str, message: &TelegramMessage) -> Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("QCOLD_TELEGRAM_CHAT_ID", message.chat.id.to_string())
        .env("QCOLD_TELEGRAM_MESSAGE_ID", message.message_id.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn meta-agent command: {command}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .context("failed to write prompt to meta-agent command")?;
    }

    let output = child
        .wait_with_output()
        .context("failed to wait for meta-agent command")?;
    if output.status.success() {
        let response = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if response.is_empty() {
            return Ok("Meta-agent returned no output.".to_string());
        }
        return Ok(response);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(format!("Meta-agent command failed: {}", stderr.trim()))
}

fn meta_agent_command(configured: Option<&str>) -> Result<String> {
    if let Some(command) = configured {
        if !command.trim().is_empty() {
            return Ok(command.to_string());
        }
    }
    let cwd = repository::active_root()?;
    Ok(default_meta_agent_command(&cwd))
}

fn default_meta_agent_command(cwd: &PathBuf) -> String {
    format!(
        "c1 exec --ephemeral --cd {} -",
        shell_quote(&cwd.display().to_string())
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

struct TelegramClient {
    config: TelegramConfig,
    agent: ureq::Agent,
}

impl TelegramClient {
    fn new(config: TelegramConfig) -> Self {
        Self {
            config,
            agent: ureq::AgentBuilder::new().build(),
        }
    }

    fn apply(&self, action: TelegramAction) -> Result<()> {
        match action {
            TelegramAction::Send(message) => self.send_message(&message),
            TelegramAction::CreateTask(request) => self.create_task_topic(&request),
        }
    }

    fn set_my_commands(&self) -> Result<()> {
        let payload = serde_json::json!({
            "commands": [
                { "command": "app", "description": "Open the Q-COLD Mini App dashboard" },
                { "command": "repos", "description": "Show connected repository context" },
                { "command": "whoami", "description": "Show your Telegram user id" },
                { "command": "status", "description": "Show repository task state" },
                { "command": "agents", "description": "Show managed agents" },
                { "command": "agent_start", "description": "Start an agent through Q-COLD" },
                { "command": "task", "description": "Create a task topic" },
                { "command": "help", "description": "Show Q-COLD help" }
            ]
        });
        let response: TelegramResponse<bool> = self
            .agent
            .post(&self.api_url("setMyCommands"))
            .send_json(payload)
            .context("Telegram setMyCommands request failed")?
            .into_json()
            .context("Telegram setMyCommands JSON decode failed")?;
        response.result()?;
        Ok(())
    }

    fn create_task_topic(&self, request: &TaskRequest) -> Result<()> {
        match self.create_forum_topic(&request.source_chat_id, &request.topic_name) {
            Ok(topic) => {
                TaskState::load()?.add(TaskRecord {
                    id: request.id.clone(),
                    chat_id: request.source_chat_id.clone(),
                    thread_id: topic.message_thread_id,
                    title: request.title.clone(),
                    description: request.description.clone(),
                    source_message_id: request.source_message_id,
                    created_at: request.created_at,
                    status: "open".to_string(),
                })?;
                append_task_system_event(
                    &request.id,
                    request.created_at,
                    &format!("created from message {}", request.source_message_id),
                )?;
                let message = SendMessage {
                    chat_id: request.source_chat_id.clone(),
                    message_thread_id: Some(topic.message_thread_id),
                    text: format!(
                        "Task {} created.\n\n{}\n\nUse this topic for task discussion.",
                        request.id, request.description
                    ),
                    reply_to_message_id: None,
                    reply_markup: None,
                };
                self.send_message(&message)?;
                self.send_message(&SendMessage {
                    chat_id: request.source_chat_id.clone(),
                    message_thread_id: None,
                    text: format!(
                        "Created task {} in topic '{}'.",
                        request.id, request.topic_name
                    ),
                    reply_to_message_id: Some(request.source_message_id),
                    reply_markup: None,
                })
            }
            Err(err) => self.send_message(&SendMessage {
                chat_id: request.source_chat_id.clone(),
                message_thread_id: None,
                text: format!("Failed to create task topic: {err:#}"),
                reply_to_message_id: Some(request.source_message_id),
                reply_markup: None,
            }),
        }
    }

    fn create_forum_topic(&self, chat_id: &str, name: &str) -> Result<ForumTopic> {
        let payload = CreateForumTopicPayload { chat_id, name };
        let response: TelegramResponse<ForumTopic> = self
            .agent
            .post(&self.api_url("createForumTopic"))
            .send_json(serde_json::to_value(payload)?)
            .context("Telegram createForumTopic request failed")?
            .into_json()
            .context("Telegram createForumTopic JSON decode failed")?;
        response.result()
    }

    fn get_updates(&self, offset: Option<i64>, timeout: u64) -> Result<Vec<TelegramUpdate>> {
        let url = self.api_url("getUpdates");
        let mut request = self
            .agent
            .get(&url)
            .query("timeout", &timeout.to_string())
            .query("allowed_updates", r#"["message"]"#);
        let offset_value;
        if let Some(offset) = offset {
            offset_value = offset.to_string();
            request = request.query("offset", &offset_value);
        }
        let response: TelegramResponse<Vec<TelegramUpdate>> = request
            .call()
            .context("Telegram getUpdates request failed")?
            .into_json()
            .context("Telegram getUpdates JSON decode failed")?;
        response.result()
    }

    fn send_message(&self, message: &SendMessage) -> Result<()> {
        if self.config.history_enabled {
            if let Err(err) = history::append("telegram", "assistant", &message.text) {
                eprintln!("Telegram history append failed: {err:#}");
            }
        }
        for text in split_telegram_text(&message.text) {
            let payload = SendMessagePayload {
                chat_id: &message.chat_id,
                text: &text,
                message_thread_id: message.message_thread_id,
                reply_to_message_id: message.reply_to_message_id,
                reply_markup: message.reply_markup.as_ref().map(ReplyMarkup::to_json),
            };
            let response: TelegramResponse<serde_json::Value> = self
                .agent
                .post(&self.api_url("sendMessage"))
                .send_json(serde_json::to_value(payload)?)
                .context("Telegram sendMessage request failed")?
                .into_json()
                .context("Telegram sendMessage JSON decode failed")?;
            response.result()?;
        }
        Ok(())
    }

    fn api_url(&self, method: &str) -> String {
        format!(
            "{}/bot{}/{}",
            self.config.api_base_url.trim_end_matches('/'),
            self.config.bot_token,
            method
        )
    }
}

fn split_telegram_text(text: &str) -> Vec<String> {
    const LIMIT: usize = 3900;
    if text.len() <= LIMIT {
        return vec![text.to_string()];
    }
    text.as_bytes()
        .chunks(LIMIT)
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect()
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

impl<T> TelegramResponse<T> {
    fn result(self) -> Result<T> {
        if self.ok {
            return self.result.context("Telegram response omitted result");
        }
        anyhow::bail!(
            "Telegram API returned error: {}",
            self.description.unwrap_or_else(|| "unknown".to_string())
        );
    }
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    message_id: i64,
    message_thread_id: Option<i64>,
    chat: TelegramChat,
    from: Option<TelegramUser>,
    text: Option<String>,
    reply_to_message: Option<Box<TelegramMessage>>,
}

impl TelegramMessage {
    fn reply(&self, text: String) -> SendMessage {
        SendMessage {
            chat_id: self.chat.id.to_string(),
            message_thread_id: self.message_thread_id,
            text,
            reply_to_message_id: Some(self.message_id),
            reply_markup: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    id: i64,
    username: Option<String>,
}

struct SendMessage {
    chat_id: String,
    message_thread_id: Option<i64>,
    text: String,
    reply_to_message_id: Option<i64>,
    reply_markup: Option<ReplyMarkup>,
}

enum ReplyMarkup {
    WebAppButton { text: String, url: String },
}

impl ReplyMarkup {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::WebAppButton { text, url } => serde_json::json!({
                "inline_keyboard": [[{
                    "text": text,
                    "web_app": { "url": url },
                }]]
            }),
        }
    }
}

#[derive(Serialize)]
struct SendMessagePayload<'a> {
    chat_id: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct CreateForumTopicPayload<'a> {
    chat_id: &'a str,
    name: &'a str,
}

#[derive(Debug, Deserialize)]
struct ForumTopic {
    message_thread_id: i64,
}

enum TelegramAction {
    Send(SendMessage),
    CreateTask(TaskRequest),
}

struct TaskRequest {
    id: String,
    source_chat_id: String,
    source_message_id: i64,
    topic_name: String,
    title: String,
    description: String,
    created_at: u64,
}

impl TaskRequest {
    fn new(message: &TelegramMessage, description: &str) -> Result<Self> {
        let description = description.trim();
        if description.is_empty() {
            bail!("usage: /task <description>");
        }
        let created_at = unix_now()?;
        let id = TaskState::load()?.next_task_id();
        let title = task_title(description);
        let topic_name = truncate_topic_name(&format!("{id} {title}"));
        Ok(Self {
            id,
            source_chat_id: message.chat.id.to_string(),
            source_message_id: message.message_id,
            topic_name,
            title,
            description: description.to_string(),
            created_at,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TaskRecord {
    id: String,
    chat_id: String,
    thread_id: i64,
    title: String,
    description: String,
    source_message_id: i64,
    created_at: u64,
    status: String,
}

struct TaskState {
    records: Vec<TaskRecord>,
}

impl TaskState {
    fn load() -> Result<Self> {
        let records = state::load_task_topics(&telegram_tasks_path()?, &task_events_dir()?)?
            .into_iter()
            .map(|row| TaskRecord {
                id: row.id,
                chat_id: row.chat_id,
                thread_id: row.thread_id,
                title: row.title,
                description: row.description,
                source_message_id: row.source_message_id,
                created_at: row.created_at,
                status: row.status,
            })
            .collect();
        Ok(Self { records })
    }

    fn add(self, record: TaskRecord) -> Result<()> {
        if self.records.iter().any(|item| item.id == record.id) {
            bail!("telegram task already exists: {}", record.id);
        }
        state::add_task_topic(&state::TaskTopicRow {
            topic_name: format!("{} {}", record.id, record.title),
            id: record.id,
            chat_id: record.chat_id,
            thread_id: record.thread_id,
            title: record.title,
            description: record.description,
            source_message_id: record.source_message_id,
            created_at: record.created_at,
            status: record.status,
        })
    }

    fn find_by_thread(&self, chat_id: i64, thread_id: i64) -> Option<TaskRecord> {
        let chat_id = chat_id.to_string();
        self.records
            .iter()
            .find(|record| record.chat_id == chat_id && record.thread_id == thread_id)
            .cloned()
    }

    fn next_task_id(&self) -> String {
        state::next_task_id(self.records.len())
            .unwrap_or_else(|_| format!("qcd-{:04}", self.records.len() + 1))
    }
}

fn append_task_event(task_id: &str, message: &TelegramMessage, text: &str) -> Result<()> {
    let user_id = message
        .from
        .as_ref()
        .map_or_else(|| "unknown".to_string(), |user| user.id.to_string());
    append_task_system_event(
        task_id,
        unix_now()?,
        &format!(
            "operator message chat={} thread={} message={} user={}: {}",
            message.chat.id,
            message.message_thread_id.unwrap_or_default(),
            message.message_id,
            user_id,
            text
        ),
    )
}

fn append_task_system_event(task_id: &str, timestamp: u64, text: &str) -> Result<()> {
    let _ = timestamp;
    state::append_event("telegram", "task.input", Some(task_id), None, None, text)
}

fn telegram_tasks_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("telegram_tasks.tsv"))
}

fn task_events_dir() -> Result<PathBuf> {
    Ok(state_dir()?.join("task-events"))
}

fn state_dir() -> Result<PathBuf> {
    if let Ok(path) = env::var("QCOLD_STATE_DIR") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    let home = env::var("HOME").context("HOME is required when QCOLD_STATE_DIR is unset")?;
    Ok(PathBuf::from(home).join(".local/state/qcold"))
}

fn task_title(description: &str) -> String {
    let title = description
        .lines()
        .next()
        .unwrap_or(description)
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        "task".to_string()
    } else {
        title
    }
}

fn truncate_topic_name(value: &str) -> String {
    const LIMIT: usize = 128;
    value.chars().take(LIMIT).collect()
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::env;

    fn config() -> TelegramConfig {
        TelegramConfig {
            api_base_url: "http://127.0.0.1".to_string(),
            bot_token: "token".to_string(),
            operator_chat_id: "100".to_string(),
            meta_chat_id: "200".to_string(),
            allowed_user_ids: BTreeSet::from([7]),
            allowed_usernames: BTreeSet::new(),
            meta_agent_command: None,
            webapp_url: None,
            history_enabled: false,
        }
    }

    fn config_with_meta_agent() -> TelegramConfig {
        let mut config = config();
        config.meta_agent_command = Some("sh -c 'cat >/dev/null; printf handled'".to_string());
        config
    }

    #[test]
    fn default_meta_agent_command_uses_c1_exec() {
        assert_eq!(
            default_meta_agent_command(&PathBuf::from("/workspace/repo")),
            "c1 exec --ephemeral --cd '/workspace/repo' -"
        );
    }

    fn message(chat_id: i64, text: &str) -> TelegramMessage {
        TelegramMessage {
            message_id: 42,
            message_thread_id: None,
            chat: TelegramChat {
                id: chat_id,
                kind: "supergroup".to_string(),
            },
            from: Some(TelegramUser {
                id: 7,
                username: Some("chttlr".to_string()),
            }),
            text: Some(text.to_string()),
            reply_to_message: None,
        }
    }

    fn send_action(action: TelegramAction) -> SendMessage {
        match action {
            TelegramAction::Send(message) => message,
            TelegramAction::CreateTask(_) => panic!("expected send action"),
        }
    }

    fn create_task_action(action: TelegramAction) -> TaskRequest {
        match action {
            TelegramAction::CreateTask(request) => request,
            TelegramAction::Send(_) => panic!("expected create task action"),
        }
    }

    #[test]
    fn direct_meta_chat_message_routes_to_meta_agent() {
        let router = Router::new(config_with_meta_agent());
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(message(200, "what are you doing?")),
        };
        let action = send_action(router.route(&update).unwrap().unwrap());
        assert_eq!(action.chat_id, "200");
        assert_eq!(action.text, "handled");
    }

    #[test]
    fn operator_plain_message_is_ignored_without_reply_context() {
        let router = Router::new(config());
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(message(100, "noise")),
        };
        assert!(router.route(&update).unwrap().is_none());
    }

    #[test]
    fn reply_in_operator_chat_routes_to_meta_agent() {
        let router = Router::new(config_with_meta_agent());
        let mut msg = message(100, "continue");
        msg.reply_to_message = Some(Box::new(message(100, "previous bot message")));
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(msg),
        };
        let action = send_action(router.route(&update).unwrap().unwrap());
        assert_eq!(action.chat_id, "100");
        assert_eq!(action.text, "handled");
    }

    #[test]
    fn unauthorized_user_is_ignored() {
        let router = Router::new(config());
        let mut msg = message(200, "hello");
        msg.from = Some(TelegramUser {
            id: 8,
            username: Some("other".to_string()),
        });
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(msg),
        };
        assert!(router.route(&update).unwrap().is_none());
    }

    #[test]
    fn allowed_user_private_chat_accepts_commands() {
        let router = Router::new(config());
        let mut msg = message(7, "/help");
        msg.chat.kind = "private".to_string();
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(msg),
        };
        let action = send_action(router.route(&update).unwrap().unwrap());
        assert!(action.text.contains("Q-COLD Telegram control plane"));
    }

    #[test]
    fn allowed_user_private_chat_routes_plain_text_to_meta_agent() {
        let router = Router::new(config_with_meta_agent());
        let mut msg = message(7, "continue the task");
        msg.chat.kind = "private".to_string();
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(msg),
        };
        let action = send_action(router.route(&update).unwrap().unwrap());
        assert_eq!(action.text, "handled");
    }

    #[test]
    fn task_command_creates_task_topic_action() {
        let router = Router::new(config());
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(message(100, "/task implement telegram topics")),
        };
        let action = create_task_action(router.route(&update).unwrap().unwrap());
        assert_eq!(action.source_chat_id, "100");
        assert_eq!(action.title, "implement telegram topics");
        assert!(action.topic_name.starts_with("qcd-"));
    }

    #[test]
    fn app_command_explains_missing_webapp_url() {
        let router = Router::new(config());
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(message(100, "/app")),
        };
        let action = send_action(router.route(&update).unwrap().unwrap());
        assert!(action.text.contains("Mini App URL is not configured"));
        assert!(action.reply_markup.is_none());
    }

    #[test]
    fn app_command_uses_webapp_button_when_configured() {
        let mut config = config();
        config.webapp_url = Some("https://qcold.example/app".to_string());
        let router = Router::new(config);
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(message(100, "/app")),
        };
        let action = send_action(router.route(&update).unwrap().unwrap());
        assert_eq!(action.text, "Open Q-COLD Mini App.");
        let markup = action.reply_markup.as_ref().unwrap().to_json();
        assert_eq!(
            markup["inline_keyboard"][0][0]["web_app"]["url"],
            "https://qcold.example/app"
        );
    }

    #[test]
    fn whoami_reports_user_id() {
        let router = Router::new(config());
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(message(100, "/whoami")),
        };
        let action = send_action(router.route(&update).unwrap().unwrap());
        assert!(action.text.contains("Telegram user id: 7"));
        assert!(action.text.contains("Operator user allowlist is active."));
    }

    #[test]
    fn username_allowlist_accepts_matching_user() {
        let mut config = config();
        config.allowed_user_ids.clear();
        config.allowed_usernames = BTreeSet::from(["chttlr".to_string()]);
        let router = Router::new(config);
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(message(100, "/whoami")),
        };
        let action = send_action(router.route(&update).unwrap().unwrap());
        assert!(action.text.contains("Username: @chttlr"));
    }

    #[test]
    fn router_unit_tests_do_not_write_shared_history() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));

        let router = Router::new(config_with_meta_agent());
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(message(200, "what are you doing?")),
        };
        let action = send_action(router.route(&update).unwrap().unwrap());

        assert_eq!(action.text, "handled");
        assert!(history::load_recent(10).unwrap().is_empty());
    }
}
