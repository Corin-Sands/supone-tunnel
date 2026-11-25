// Author: Corin-Sands
//! 本机工具环：Hub phllm_ → OpenAI 兼容口 → 本地 dispatch。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::redact::Redactor;
use crate::shell::Shell;
use crate::trace::Trace;

const SYSTEM: &str = "\
你是本机 mihomo 壳的控制助手。只能通过工具操作，不要输出 YAML。\n\
ip_、sec_、url_、host_ 后接 8 位十六进制是代号，必须原样复制到工具参数里。\n\
不要编写、猜测或补全 IP、域名、密钥和订阅地址。\n\
节点名必须来自 list_nodes / list_local_proxies / 状态列表。模式只能是 rule、global、direct。\n\
观测：dns_lookup、list_rules、list_connections、explain_route、probe_url、list_nodes、list_groups、diagnose、net_overview、net_interfaces、net_routes。\n\
问某个网站走直连还是代理时，必须先调用 explain_route；要验证当前配置下站点是否可达用 probe_url（链路只信连接日志，不推理）。TUN/其它 VPN 冲突先 net_overview。\n\
本地叶子出口：upsert_proxy（socks5/http/https；本机 SOCKS 可用 127.0.0.1）+ prepend_rule；本地代理组：upsert_proxy_group。DNS：set_dns。清空覆盖：reset_override。\n\
出口可选 TUN 或系统代理（set_tun / set_system_proxy）；开系统代理前须内核已运行。\n\
其它 VPN 叠网或 VPN 出口 IP 不要进 TUN 时用 add_tun_route_exclude（写 route-exclude-address），不要用 prepend_rule / IP-CIDR 规则冒充。\n\
订阅 proxies[].server 字面量 IP 会在 rebuild 时自动进 route-exclude-address；SSH/管理节点不要用 prepend_rule。\n\
对话只附带最近一段记录。做完用一句中文说明结果。";

const HISTORY_LIMIT: usize = 30;
const MODEL_TEXT_LIMIT: usize = 2500;
const TITLE_LIMIT: usize = 40;
/// 单次用户消息内「模型 → 工具 → 再问模型」的最大轮次（每轮可含多次工具调用）。
/// Author: Corin-Sands
const MAX_TOOL_ROUNDS: usize = 64;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiMessage {
    pub role: String,
    pub text: String,
}

/// 对话列表项（侧栏/下拉）。Author: Corin-Sands
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub active: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record {
    User { text: String },
    Assistant { text: String },
    Tools { preface: String, calls: Vec<StoredCall> },
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredCall {
    id: String,
    name: String,
    args: Value,
    result: String,
}

pub struct Session {
    redactor: Redactor,
    records: Vec<Record>,
    /// `app_data/conversations/`
    root: PathBuf,
    id: String,
    path: PathBuf,
    /// 本机会话键（历史字段；归因已由 Hub 钥承载）。
    conversation_user: String,
    meta_path: PathBuf,
    title: String,
    created_at: String,
    updated_at: String,
    /// 兼容旧 meta；工具环不再用快照指纹。Author: Corin-Sands
    last_context_hash: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SessionMeta {
    #[serde(default)]
    conversation_user: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    last_context_hash: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConversationIndex {
    #[serde(default)]
    active_id: String,
}

impl Session {
    pub fn open(app_dir: PathBuf) -> Self {
        let root = app_dir.join("conversations");
        let _ = fs::create_dir_all(&root);
        migrate_legacy(&app_dir, &root);

        let index = load_index(&root);
        let id = if !index.active_id.trim().is_empty() && conversation_dir(&root, &index.active_id).is_dir()
        {
            index.active_id
        } else if let Some(id) = newest_conversation_id(&root) {
            id
        } else {
            new_conversation_id()
        };

        let session = Self::load_into(root, id);
        session.persist_index();
        session.save_meta();
        session
    }

    fn load_into(root: PathBuf, id: String) -> Self {
        let dir = conversation_dir(&root, &id);
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("transcript.jsonl");
        let meta_path = dir.join("meta.json");
        let records = load_records(&path);
        let meta = load_meta(&meta_path);
        let now = now_stamp();
        let conversation_user = if meta.conversation_user.trim().is_empty() {
            new_conversation_user()
        } else {
            meta.conversation_user
        };
        let title = if meta.title.trim().is_empty() {
            title_from_records(&records)
        } else {
            meta.title
        };
        let created_at = if meta.created_at.trim().is_empty() {
            now.clone()
        } else {
            meta.created_at
        };
        let updated_at = if meta.updated_at.trim().is_empty() {
            now
        } else {
            meta.updated_at
        };
        Self {
            redactor: Redactor::new(),
            records,
            root,
            id,
            path,
            conversation_user,
            meta_path,
            title,
            created_at,
            updated_at,
            last_context_hash: meta.last_context_hash,
        }
    }

    pub fn conversation_user(&self) -> &str {
        &self.conversation_user
    }

    pub fn list_conversations(&self) -> Vec<ConversationSummary> {
        let mut items = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return items;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(id) = path.file_name().and_then(|name| name.to_str()).map(str::to_string) else {
                continue;
            };
            let meta = load_meta(&path.join("meta.json"));
            let records = load_records(&path.join("transcript.jsonl"));
            let title = if meta.title.trim().is_empty() {
                title_from_records(&records)
            } else {
                meta.title
            };
            let updated_at = if meta.updated_at.trim().is_empty() {
                meta.created_at
            } else {
                meta.updated_at
            };
            items.push(ConversationSummary {
                id: id.clone(),
                title,
                updated_at,
                active: id == self.id,
            });
        }
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        items
    }

    /// 开新对话：有内容则保留当前归档，切到空白会话。Author: Corin-Sands
    pub fn new_conversation(&mut self) {
        if self.records.is_empty() {
            self.conversation_user = new_conversation_user();
            self.title = "新对话".into();
            self.updated_at = now_stamp();
            self.last_context_hash.clear();
            self.save_meta();
            self.persist_index();
            return;
        }
        self.save();
        self.save_meta();
        let id = new_conversation_id();
        let now = now_stamp();
        *self = Self {
            redactor: Redactor::new(),
            records: Vec::new(),
            root: self.root.clone(),
            id,
            path: PathBuf::new(),
            conversation_user: new_conversation_user(),
            meta_path: PathBuf::new(),
            title: "新对话".into(),
            created_at: now.clone(),
            updated_at: now,
            last_context_hash: String::new(),
        };
        let dir = conversation_dir(&self.root, &self.id);
        let _ = fs::create_dir_all(&dir);
        self.path = dir.join("transcript.jsonl");
        self.meta_path = dir.join("meta.json");
        self.save();
        self.save_meta();
        self.persist_index();
    }

    pub fn open_conversation(&mut self, id: &str) -> Result<(), String> {
        let id = id.trim();
        if id.is_empty() {
            return Err("对话 id 无效".into());
        }
        if id == self.id {
            return Ok(());
        }
        let dir = conversation_dir(&self.root, id);
        if !dir.is_dir() {
            return Err("对话不存在".into());
        }
        self.save();
        self.save_meta();
        *self = Self::load_into(self.root.clone(), id.to_string());
        self.persist_index();
        self.save_meta();
        Ok(())
    }

    pub fn ui_messages(&self) -> Vec<UiMessage> {
        let mut messages = Vec::new();
        for record in &self.records {
            match record {
                Record::User { text } => messages.push(UiMessage {
                    role: "user".into(),
                    text: text.clone(),
                }),
                Record::Assistant { text } => messages.push(UiMessage {
                    role: "assistant".into(),
                    text: text.clone(),
                }),
                Record::Tools { calls, .. } => {
                    for call in calls {
                        messages.push(UiMessage {
                            role: "tool".into(),
                            text: format!("{} → {}", call.name, call.result),
                        });
                    }
                }
            }
        }
        messages
    }

    fn touch_title_from_user(&mut self, text: &str) {
        if self.title.trim().is_empty() || self.title == "新对话" {
            self.title = truncate_title(text);
        }
        self.updated_at = now_stamp();
    }

    fn save(&self) {
        let mut text = String::new();
        for record in &self.records {
            if let Ok(line) = serde_json::to_string(record) {
                text.push_str(&line);
                text.push('\n');
            }
        }
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("tmp");
        if fs::write(&tmp, text.as_bytes()).is_ok() {
            let _ = fs::rename(&tmp, &self.path);
        }
    }

    fn save_meta(&self) {
        let meta = SessionMeta {
            conversation_user: self.conversation_user.clone(),
            title: self.title.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            last_context_hash: self.last_context_hash.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&meta) {
            if let Some(parent) = self.meta_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let tmp = self.meta_path.with_extension("tmp");
            if fs::write(&tmp, text.as_bytes()).is_ok() {
                let _ = fs::rename(&tmp, &self.meta_path);
            }
        }
    }

    fn persist_index(&self) {
        let index = ConversationIndex {
            active_id: self.id.clone(),
        };
        let path = self.root.join("index.json");
        if let Ok(text) = serde_json::to_string_pretty(&index) {
            let tmp = path.with_extension("tmp");
            if fs::write(&tmp, text.as_bytes()).is_ok() {
                let _ = fs::rename(&tmp, &path);
            }
        }
    }
}

fn new_conversation_user() -> String {
    format!("supone-{}", uuid::Uuid::new_v4())
}

fn new_conversation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn conversation_dir(root: &Path, id: &str) -> PathBuf {
    root.join(id)
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn truncate_title(text: &str) -> String {
    let trimmed = text.trim().replace('\n', " ");
    if trimmed.is_empty() {
        return "新对话".into();
    }
    let mut out: String = trimmed.chars().take(TITLE_LIMIT).collect();
    if trimmed.chars().count() > TITLE_LIMIT {
        out.push('…');
    }
    out
}

fn title_from_records(records: &[Record]) -> String {
    for record in records {
        if let Record::User { text } = record {
            return truncate_title(text);
        }
    }
    "新对话".into()
}

fn load_index(root: &Path) -> ConversationIndex {
    let path = root.join("index.json");
    let Ok(text) = fs::read_to_string(path) else {
        return ConversationIndex::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn newest_conversation_id(root: &Path) -> Option<String> {
    let mut best: Option<(String, String)> = None;
    let Ok(entries) = fs::read_dir(root) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(id) = path.file_name().and_then(|name| name.to_str()).map(str::to_string) else {
            continue;
        };
        let meta = load_meta(&path.join("meta.json"));
        let stamp = if meta.updated_at.trim().is_empty() {
            meta.created_at
        } else {
            meta.updated_at
        };
        match &best {
            Some((_, prev)) if prev >= &stamp => {}
            _ => best = Some((id, stamp)),
        }
    }
    best.map(|(id, _)| id)
}

/// 把旧版根目录 transcript/conversation 迁到 conversations/。Author: Corin-Sands
fn migrate_legacy(app_dir: &Path, root: &Path) {
    let legacy_transcript = app_dir.join("transcript.jsonl");
    let legacy_meta = app_dir.join("conversation.json");
    if !legacy_transcript.exists() && !legacy_meta.exists() {
        return;
    }
    if newest_conversation_id(root).is_some() {
        return;
    }
    let id = new_conversation_id();
    let dir = conversation_dir(root, &id);
    let _ = fs::create_dir_all(&dir);
    let records = load_records(&legacy_transcript);
    let legacy = load_meta(&legacy_meta);
    let now = now_stamp();
    let meta = SessionMeta {
        conversation_user: if legacy.conversation_user.trim().is_empty() {
            new_conversation_user()
        } else {
            legacy.conversation_user
        },
        title: title_from_records(&records),
        created_at: now.clone(),
        updated_at: now,
        last_context_hash: legacy.last_context_hash,
    };
    let new_transcript = dir.join("transcript.jsonl");
    let new_meta = dir.join("meta.json");
    if legacy_transcript.exists() {
        let _ = fs::copy(&legacy_transcript, &new_transcript);
    } else {
        let _ = fs::write(&new_transcript, b"");
    }
    if let Ok(text) = serde_json::to_string_pretty(&meta) {
        let _ = fs::write(&new_meta, text.as_bytes());
    }
    let index = ConversationIndex { active_id: id };
    if let Ok(text) = serde_json::to_string_pretty(&index) {
        let _ = fs::write(root.join("index.json"), text.as_bytes());
    }
    let _ = fs::rename(
        &legacy_transcript,
        app_dir.join("transcript.jsonl.migrated"),
    );
    let _ = fs::rename(&legacy_meta, app_dir.join("conversation.json.migrated"));
}

fn load_meta(path: &Path) -> SessionMeta {
    let Ok(text) = fs::read_to_string(path) else {
        return SessionMeta::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub async fn chat(
    shell: &Shell,
    session: &mut Session,
    text: &str,
    cancel: &mut watch::Receiver<bool>,
    mut on_progress: impl FnMut(&[UiMessage]),
    mut on_trace: impl FnMut(&str, &str, &str),
) -> Result<Vec<UiMessage>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("请输入内容".into());
    }
    let trace = Trace::start(shell.data_root(), "agent.chat");
    on_trace(&trace.id, "start", "agent.chat");
    let outcome = chat_traced(
        shell,
        session,
        text,
        cancel,
        &trace,
        &mut on_progress,
        &mut on_trace,
    )
    .await;
    match outcome {
        Ok(messages) => {
            trace.event("done", "ok");
            on_trace(&trace.id, "done", "ok");
            Ok(messages)
        }
        Err(err) => {
            let annotated = if err.contains("traceId=") {
                err
            } else {
                trace.fail("agent.chat", &err)
            };
            on_trace(&trace.id, "fail", &annotated);
            Err(annotated)
        }
    }
}

async fn chat_traced(
    shell: &Shell,
    session: &mut Session,
    text: &str,
    cancel: &mut watch::Receiver<bool>,
    trace: &Trace,
    on_progress: &mut impl FnMut(&[UiMessage]),
    on_trace: &mut impl FnMut(&str, &str, &str),
) -> Result<Vec<UiMessage>, String> {
    ensure_not_cancelled(cancel).map_err(|err| trace.fail("cancel", err))?;
    // 订阅 token → Hub 换 phllm_ → supone-llm；工具在本机执行。Author: Corin-Sands
    trace.event("agent.settings", "begin");
    on_trace(&trace.id, "agent.settings", "begin");
    let settings = shell
        .agent_settings()
        .await
        .map_err(|err| trace.fail("agent.settings", err))?;
    trace.event(
        "agent.settings",
        &format!("ok base={} model={}", settings.base, settings.model),
    );
    on_trace(
        &trace.id,
        "agent.settings",
        &format!("ok model={}", settings.model),
    );

    session.records.push(Record::User {
        text: text.to_string(),
    });
    session.touch_title_from_user(text);
    session.save();
    session.save_meta();
    on_progress(&session.ui_messages());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|err| trace.fail("http.client", err.to_string()))?;
    for round in 1..=MAX_TOOL_ROUNDS {
        ensure_not_cancelled(cancel).map_err(|err| trace.fail("cancel", err))?;
        trace.event("round", &format!("n={round}"));
        on_trace(&trace.id, "round", &format!("n={round}"));
        let messages = build_messages(shell, session)
            .await
            .map_err(|err| trace.fail("build_messages", err))?;
        let response = post_chat(&client, &settings, messages, cancel, trace).await?;
        if let Some(calls) = response.tool_calls {
            if !calls.is_empty() {
                let mut stored = Vec::new();
                for call in calls {
                    ensure_not_cancelled(cancel).map_err(|err| trace.fail("cancel", err))?;
                    let raw = if call.function.arguments.trim().is_empty() {
                        "{}".to_string()
                    } else {
                        call.function.arguments
                    };
                    let (args, result) = match session.redactor.restore_tool_args(&raw) {
                        Ok(args) => {
                            let name = call.function.name.clone();
                            trace.event("tool.begin", &format!("name={name}"));
                            on_trace(&trace.id, "tool.begin", &name);
                            let mut result = dispatch(shell, &name, &args).await;
                            if result.starts_with("失败:") {
                                result = trace.annotate(&result);
                                trace.event("tool.end", &format!("name={name} fail"));
                                on_trace(&trace.id, "tool.end", &format!("{name} fail"));
                            } else {
                                trace.event("tool.end", &format!("name={name} ok"));
                                on_trace(&trace.id, "tool.end", &format!("{name} ok"));
                            }
                            (args, result)
                        }
                        Err(err) => {
                            let msg = trace.fail("tool.args", err);
                            (json!({}), format!("失败: {msg}"))
                        }
                    };
                    stored.push(StoredCall {
                        id: call.id,
                        name: call.function.name,
                        args,
                        result,
                    });
                }
                let preface = response.content.unwrap_or_default();
                session.records.push(Record::Tools {
                    preface: session.redactor.unmask(&preface),
                    calls: stored,
                });
                session.updated_at = now_stamp();
                session.save();
                session.save_meta();
                // 每轮工具落盘后推给 UI，避免整轮结束才一次性刷新。Author: Corin-Sands
                on_progress(&session.ui_messages());
                continue;
            }
        }
        let reply = session
            .redactor
            .unmask(&response.content.unwrap_or_default());
        let reply = if reply.trim().is_empty() {
            "已执行。".to_string()
        } else {
            reply
        };
        session.records.push(Record::Assistant { text: reply });
        session.updated_at = now_stamp();
        session.save();
        session.save_meta();
        let messages = session.ui_messages();
        on_progress(&messages);
        return Ok(messages);
    }
    let capped = format!("已达到 {MAX_TOOL_ROUNDS} 步上限");
    trace.event("round.cap", &capped);
    session.records.push(Record::Assistant {
        text: trace.annotate(&capped),
    });
    session.updated_at = now_stamp();
    session.save();
    session.save_meta();
    let messages = session.ui_messages();
    on_progress(&messages);
    Ok(messages)
}

fn ensure_not_cancelled(cancel: &watch::Receiver<bool>) -> Result<(), String> {
    if *cancel.borrow() {
        Err("已取消".into())
    } else {
        Ok(())
    }
}

async fn with_cancel<T>(
    cancel: &mut watch::Receiver<bool>,
    fut: impl std::future::Future<Output = T>,
) -> Result<T, String> {
    ensure_not_cancelled(cancel)?;
    tokio::select! {
        result = fut => Ok(result),
        changed = cancel.changed() => {
            let _ = changed;
            if *cancel.borrow() {
                Err("已取消".into())
            } else {
                Err("已取消".into())
            }
        }
    }
}

async fn dispatch(shell: &Shell, name: &str, args: &Value) -> String {
    let outcome = match name {
        "list_profiles" => shell.list_text(),
        "import_subscription" => {
            let url = match arg_str(args, "url") {
                Ok(url) => url.to_string(),
                Err(err) => return format!("失败: {err}"),
            };
            let name = arg_str(args, "name").ok().map(str::to_string);
            shell.import_subscription(&url, name).await
        }
        "refresh_subscription" => match arg_str(args, "id") {
            Ok(id) => shell.refresh_subscription(id).await,
            Err(err) => Err(err),
        },
        "activate_profile" => match arg_str(args, "id") {
            Ok(id) => shell.activate_profile(id).await,
            Err(err) => Err(err),
        },
        "reset_override" => {
            let id = args.get("id").and_then(|v| v.as_str());
            shell.reset_override(id).await
        }
        "start_core" => shell.start().await.map(|_| "内核已启动".to_string()),
        "stop_core" => shell.stop().await.map(|_| "内核已停止".into()),
        "get_status" => shell.model_status().await,
        "diagnose" => shell.diagnose().await,
        "set_mode" => match arg_str(args, "mode") {
            Ok(mode) => shell.set_mode(mode).await,
            Err(err) => Err(err),
        },
        "select_proxy" => {
            let group = match arg_str(args, "group") {
                Ok(group) => group.to_string(),
                Err(err) => return format!("失败: {err}"),
            };
            let name = match arg_str(args, "name") {
                Ok(name) => name.to_string(),
                Err(err) => return format!("失败: {err}"),
            };
            shell.select_proxy(&group, &name).await
        }
        "list_local_proxies" => Ok(shell.local_proxies_text()),
        "upsert_proxy" => {
            let name = match arg_str(args, "name") {
                Ok(name) => name.to_string(),
                Err(err) => return format!("失败: {err}"),
            };
            let proxy_type = match arg_str(args, "type") {
                Ok(kind) => kind.to_string(),
                Err(err) => return format!("失败: {err}"),
            };
            let server = match arg_str(args, "server") {
                Ok(server) => server.to_string(),
                Err(err) => return format!("失败: {err}"),
            };
            let port = match arg_u16(args, "port") {
                Ok(port) => port,
                Err(err) => return format!("失败: {err}"),
            };
            let username = args.get("username").and_then(|v| v.as_str());
            let password = args.get("password").and_then(|v| v.as_str());
            shell
                .upsert_proxy(&name, &proxy_type, &server, port, username, password)
                .await
        }
        "remove_proxy" => match arg_str(args, "name") {
            Ok(name) => shell.remove_proxy(name).await,
            Err(err) => Err(err),
        },
        "upsert_proxy_group" => {
            let name = match arg_str(args, "name") {
                Ok(name) => name.to_string(),
                Err(err) => return format!("失败: {err}"),
            };
            let group_type = args
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("select");
            let proxies = match arg_string_list(args, "proxies") {
                Ok(list) => list,
                Err(err) => return format!("失败: {err}"),
            };
            let url = args.get("url").and_then(|v| v.as_str());
            let interval = args.get("interval").and_then(|v| v.as_u64());
            shell
                .upsert_proxy_group(&name, group_type, &proxies, url, interval)
                .await
        }
        "set_mixed_port" => match arg_u16(args, "port") {
            Ok(port) => shell.set_mixed_port(port).await,
            Err(err) => Err(err),
        },
        "set_tun" => match arg_bool(args, "enable") {
            Ok(enable) => shell.set_tun(enable).await,
            Err(err) => Err(err),
        },
        "list_tun_route_excludes" => Ok(shell.tun_route_excludes_text()),
        "add_tun_route_exclude" => match arg_str(args, "cidr") {
            Ok(cidr) => shell.add_tun_route_exclude(cidr).await,
            Err(err) => Err(err),
        },
        "remove_tun_route_exclude" => match arg_str(args, "cidr") {
            Ok(cidr) => shell.remove_tun_route_exclude(cidr).await,
            Err(err) => Err(err),
        },
        "set_system_proxy" => match arg_bool(args, "enable") {
            Ok(enable) => shell.set_system_proxy(enable).await,
            Err(err) => Err(err),
        },
        "set_allow_lan" => match arg_bool(args, "enable") {
            Ok(enable) => shell.set_allow_lan(enable).await,
            Err(err) => Err(err),
        },
        "dns_config" => Ok(shell.dns_config_text()),
        "set_dns" => match json_to_yaml(args.get("dns").cloned().unwrap_or(Value::Null)) {
            Ok(patch) => shell.set_dns(patch).await,
            Err(err) => Err(err),
        },
        "list_prepend_rules" => Ok(shell.prepend_rules_text()),
        "prepend_rule" => match arg_str(args, "rule") {
            Ok(rule) => shell.prepend_rule(rule).await,
            Err(err) => Err(err),
        },
        "remove_prepend_rule" => match arg_str(args, "rule") {
            Ok(rule) => shell.remove_prepend_rule(rule).await,
            Err(err) => Err(err),
        },
        "list_nodes" => shell.nodes_text().await,
        "list_groups" => shell.groups_text().await,
        "explain_route" => match arg_str(args, "host") {
            Ok(host) => shell.explain_route(host).await,
            Err(err) => Err(err),
        },
        "probe_url" => match arg_str(args, "url") {
            Ok(url) => {
                let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64());
                shell.probe_url_text(url, timeout_ms).await
            }
            Err(err) => Err(err),
        },
        "dns_lookup" => match arg_str(args, "host") {
            Ok(host) => shell.dns_lookup_text(host).await,
            Err(err) => Err(err),
        },
        "list_rules" => shell.list_rules_text().await,
        "list_connections" => {
            let host = arg_str(args, "host").ok();
            shell.list_connections_text(host).await
        },
        "net_overview" => shell.net_overview().await,
        "net_interfaces" => Ok(shell.net_interfaces()),
        "net_routes" => Ok(shell.net_routes()),
        other => Err(format!("未知工具 {other}")),
    };
    match outcome {
        Ok(text) => text,
        Err(err) => format!("失败: {err}"),
    }
}

async fn build_messages(shell: &Shell, session: &mut Session) -> Result<Vec<Value>, String> {
    learn(shell, &mut session.redactor);
    let status = shell.model_status().await?;
    let status = session.redactor.mask_text(&status);
    let mut messages = vec![json!({
        "role": "system",
        "content": format!("{SYSTEM}\n\n当前状态:\n{status}")
    })];
    let start = session.records.len().saturating_sub(HISTORY_LIMIT);
    let records = session.records[start..].to_vec();
    for record in &records {
        match record {
            Record::User { text } => messages.push(json!({
                "role": "user",
                "content": session.redactor.mask_text(text)
            })),
            Record::Assistant { text } => messages.push(json!({
                "role": "assistant",
                "content": session.redactor.mask_text(text)
            })),
            Record::Tools { preface, calls } => {
                let tool_calls = calls
                    .iter()
                    .map(|call| {
                        let masked = session.redactor.mask_value(call.args.clone());
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": serde_json::to_string(&masked).unwrap_or_else(|_| "{}".into())
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                let content = if preface.is_empty() {
                    Value::Null
                } else {
                    Value::String(session.redactor.mask_text(preface))
                };
                messages.push(json!({
                    "role": "assistant",
                    "content": content,
                    "tool_calls": tool_calls
                }));
                for call in calls {
                    let mut result = call.result.clone();
                    if result.chars().count() > MODEL_TEXT_LIMIT {
                        result = result.chars().take(MODEL_TEXT_LIMIT).collect();
                        result.push_str("…已截断");
                    }
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": call.id,
                        "content": session.redactor.mask_text(&result)
                    }));
                }
            }
        }
    }
    Ok(messages)
}

fn learn(shell: &Shell, redactor: &mut Redactor) {
    let sources = shell.redact_sources();
    redactor.learn_secret(&sources.secret);
    for url in sources.urls {
        redactor.learn_url(&url);
    }
    for yaml in sources.yaml_bodies {
        redactor.learn_yaml(&yaml);
    }
}

pub struct AgentSettings {
    pub base: String,
    pub key: String,
    pub model: String,
    /// Hub users.id（换钥响应；工具环不另编 user 字段）。Author: Corin-Sands
    pub hub_user_id: i64,
}

fn completions_url(base: &str) -> String {
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

async fn post_chat(
    client: &reqwest::Client,
    settings: &AgentSettings,
    messages: Vec<Value>,
    cancel: &mut watch::Receiver<bool>,
    trace: &Trace,
) -> Result<AssistantMessage, String> {
    let url = completions_url(&settings.base);
    let body = json!({
        "model": settings.model,
        "temperature": 0,
        "max_tokens": 1024,
        "stream": false,
        "messages": messages,
        "tools": tool_schemas()
    });
    trace.event(
        "llm.request",
        &format!(
            "url={url} model={} messages={}",
            settings.model,
            messages.len()
        ),
    );
    let request = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", settings.key))
        .header("X-Trace-Id", &trace.id)
        .header("X-Client", "supone-tunnel")
        .json(&body)
        .send();
    let response = match with_cancel(cancel, request).await {
        Err(err) => return Err(trace.fail("llm.cancel", err)),
        Ok(Err(err)) => {
            return Err(trace.fail("llm.transport", format!("模型请求失败: {err}")));
        }
        Ok(Ok(response)) => response,
    };
    let status = response.status();
    let text = match with_cancel(cancel, response.text()).await {
        Err(err) => return Err(trace.fail("llm.cancel", err)),
        Ok(Err(err)) => return Err(trace.fail("llm.body", err.to_string())),
        Ok(Ok(text)) => text,
    };
    if !status.is_success() {
        return Err(trace.fail(
            "llm.http",
            format!("模型请求失败: HTTP {status} {}", short(&text)),
        ));
    }
    trace.event("llm.response", &format!("http={status} bytes={}", text.len()));
    let parsed: ChatResponse = serde_json::from_str(&text)
        .map_err(|err| trace.fail("llm.parse", format!("模型响应无效: {err}")))?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message)
        .ok_or_else(|| trace.fail("llm.empty", "模型响应没有内容"))
}

fn short(text: &str) -> String {
    text.chars().take(500).collect()
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("缺少 {key}"))
}

fn arg_bool(args: &Value, key: &str) -> Result<bool, String> {
    args.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("缺少 {key}"))
}

fn arg_u16(args: &Value, key: &str) -> Result<u16, String> {
    let number = args
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("缺少 {key}"))?;
    u16::try_from(number).map_err(|_| "端口无效".to_string())
}

fn arg_string_list(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let items = args
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("缺少 {key}"))?;
    if items.is_empty() {
        return Err(format!("{key} 不能为空"));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = item.as_str().map(str::trim).filter(|v| !v.is_empty()) else {
            return Err(format!("{key} 须是非空字符串数组"));
        };
        out.push(text.to_string());
    }
    Ok(out)
}

fn json_to_yaml(value: Value) -> Result<serde_yaml::Value, String> {
    let text = serde_json::to_string(&value).map_err(|err| err.to_string())?;
    serde_yaml::from_str(&text).map_err(|err| format!("dns 参数无效: {err}"))
}

fn load_records(path: &Path) -> Vec<Record> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: AssistantMessage,
}

#[derive(Deserialize)]
struct AssistantMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Deserialize)]
struct ToolCall {
    id: String,
    function: FunctionCall,
}

#[derive(Deserialize)]
struct FunctionCall {
    name: String,
    arguments: String,
}

fn tool_schemas() -> Vec<Value> {
    vec![
        tool(
            "list_profiles",
            "列出订阅 id 和名称",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "import_subscription",
            "导入订阅。url 使用代号或状态中出现的地址",
            json!({
                "type":"object",
                "properties":{
                    "url":{"type":"string"},
                    "name":{"type":"string"}
                },
                "required":["url"]
            }),
        ),
        tool(
            "refresh_subscription",
            "按 id 刷新订阅",
            json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}),
        ),
        tool(
            "activate_profile",
            "切换当前订阅",
            json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}),
        ),
        tool(
            "reset_override",
            "清空本地覆盖，回到仅订阅原文",
            json!({"type":"object","properties":{"id":{"type":"string"}}}),
        ),
        tool("start_core", "启动 mihomo", json!({"type":"object","properties":{}})),
        tool("stop_core", "停止 mihomo", json!({"type":"object","properties":{}})),
        tool("get_status", "读取当前短状态", json!({"type":"object","properties":{}})),
        tool("diagnose", "本机现场诊断", json!({"type":"object","properties":{}})),
        tool(
            "set_mode",
            "设置模式并写入覆盖",
            json!({
                "type":"object",
                "properties":{"mode":{"type":"string","enum":["rule","global","direct"]}},
                "required":["mode"]
            }),
        ),
        tool(
            "select_proxy",
            "在已有代理组中选择节点",
            json!({
                "type":"object",
                "properties":{"group":{"type":"string"},"name":{"type":"string"}},
                "required":["group","name"]
            }),
        ),
        tool(
            "list_local_proxies",
            "列出本地叶子节点覆盖",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "upsert_proxy",
            "新建或按名编辑本地叶子节点（socks5/http/https）。本机 SOCKS 可用 127.0.0.1；代号须原样复制，勿猜 IP。",
            json!({
                "type":"object",
                "properties":{
                    "name":{"type":"string"},
                    "type":{"type":"string","enum":["socks5","http","https"]},
                    "server":{"type":"string"},
                    "port":{"type":"integer","minimum":1,"maximum":65535},
                    "username":{"type":"string"},
                    "password":{"type":"string"}
                },
                "required":["name","type","server","port"]
            }),
        ),
        tool(
            "remove_proxy",
            "删除一条本地叶子节点覆盖",
            json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
        ),
        tool(
            "upsert_proxy_group",
            "新建或按名编辑代理组；成员来自 list_nodes / 已有组 / DIRECT|REJECT",
            json!({
                "type":"object",
                "properties":{
                    "name":{"type":"string"},
                    "type":{"type":"string","enum":["select","url-test","fallback","load-balance"]},
                    "proxies":{"type":"array","items":{"type":"string"},"minItems":1},
                    "url":{"type":"string"},
                    "interval":{"type":"integer","minimum":1,"maximum":86400}
                },
                "required":["name","proxies"]
            }),
        ),
        tool(
            "set_mixed_port",
            "设置 mixed 端口",
            json!({"type":"object","properties":{"port":{"type":"integer"}},"required":["port"]}),
        ),
        tool(
            "set_tun",
            "开关 TUN",
            json!({"type":"object","properties":{"enable":{"type":"boolean"}},"required":["enable"]}),
        ),
        tool(
            "list_tun_route_excludes",
            "列出 tun.route-exclude-address（不进 TUN 的网段）",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "add_tun_route_exclude",
            "把 CIDR/IP 写入 tun.route-exclude-address，决定该网段不进 TUN；不是进 TUN 后的规则",
            json!({"type":"object","properties":{"cidr":{"type":"string"}},"required":["cidr"]}),
        ),
        tool(
            "remove_tun_route_exclude",
            "从 tun.route-exclude-address 删除一条",
            json!({"type":"object","properties":{"cidr":{"type":"string"}},"required":["cidr"]}),
        ),
        tool(
            "set_system_proxy",
            "开关系统代理（指向 mixed 口）",
            json!({"type":"object","properties":{"enable":{"type":"boolean"}},"required":["enable"]}),
        ),
        tool(
            "set_allow_lan",
            "开关 allow-lan",
            json!({"type":"object","properties":{"enable":{"type":"boolean"}},"required":["enable"]}),
        ),
        tool("dns_config", "读取当前生效 DNS", json!({"type":"object","properties":{}})),
        tool(
            "set_dns",
            "调整 DNS 并写入覆盖",
            json!({
                "type":"object",
                "properties":{
                    "dns":{
                        "type":"object",
                        "properties":{
                            "enable":{"type":"boolean"},
                            "enhanced-mode":{"type":"string","enum":["fake-ip","redir-host"]},
                            "respect-rules":{"type":"boolean"},
                            "ipv6":{"type":"boolean"},
                            "nameserver":{"type":"array","items":{"type":"string"}},
                            "fallback":{"type":"array","items":{"type":"string"}},
                            "default-nameserver":{"type":"array","items":{"type":"string"}},
                            "fake-ip-filter":{"type":"array","items":{"type":"string"}},
                            "nameserver-policy":{"type":"object","additionalProperties":{"type":"array","items":{"type":"string"}}}
                        }
                    }
                },
                "required":["dns"]
            }),
        ),
        tool(
            "list_prepend_rules",
            "列出本地前置规则",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "prepend_rule",
            "在订阅规则最前插入一条，形如 DOMAIN-SUFFIX,jd.com,DIRECT 或指向本地节点",
            json!({"type":"object","properties":{"rule":{"type":"string"}},"required":["rule"]}),
        ),
        tool(
            "remove_prepend_rule",
            "删除一条本地前置规则",
            json!({"type":"object","properties":{"rule":{"type":"string"}},"required":["rule"]}),
        ),
        tool("list_nodes", "列出全部叶子节点名", json!({"type":"object","properties":{}})),
        tool("list_groups", "列出代理组与当前选中", json!({"type":"object","properties":{}})),
        tool(
            "explain_route",
            "判定域名出口：规则 + DNS + 该 IP 的系统路由 + 组的当前节点",
            json!({
                "type":"object",
                "properties":{"host":{"type":"string"}},
                "required":["host"]
            }),
        ),
        tool(
            "probe_url",
            "经 mixed 探测目标 URL（当前生效配置）；链路只来自 /connections 连接日志，不做规则推理",
            json!({
                "type":"object",
                "properties":{
                    "url":{"type":"string"},
                    "timeout_ms":{"type":"integer","minimum":1000,"maximum":30000}
                },
                "required":["url"]
            }),
        ),
        tool(
            "dns_lookup",
            "解析域名 A/AAAA，并标注 GEOIP 国家码",
            json!({
                "type":"object",
                "properties":{"host":{"type":"string"}},
                "required":["host"]
            }),
        ),
        tool(
            "list_rules",
            "列出当前生效规则（最多 120 条）",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "list_connections",
            "列出当前连接；可按 host 过滤",
            json!({
                "type":"object",
                "properties":{"host":{"type":"string"}}
            }),
        ),
        tool(
            "net_overview",
            "本机默认路由、TUN、其它 VPN 冲突摘要",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "net_interfaces",
            "活跃网卡列表",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "net_routes",
            "IPv4 路由摘要",
            json!({"type":"object","properties":{}}),
        ),
    ]
}

fn tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": { "name": name, "description": description, "parameters": parameters }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_history_keeps_prior_on_new() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = Session::open(dir.path().to_path_buf());
        session.records.push(Record::User {
            text: "把模式改成规则".into(),
        });
        session.touch_title_from_user("把模式改成规则");
        session.save();
        session.save_meta();

        session.new_conversation();
        assert!(session.records.is_empty());
        let list = session.list_conversations();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|item| item.title.contains("模式")));
        assert!(list.iter().any(|item| item.active));
    }

    #[test]
    fn migrate_legacy_root_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = dir.path();
        fs::write(
            app.join("transcript.jsonl"),
            r#"{"type":"user","text":"旧对话"}"#.as_bytes(),
        )
        .unwrap();
        fs::write(
            app.join("conversation.json"),
            r#"{"conversationUser":"supone-legacy"}"#,
        )
        .unwrap();
        let session = Session::open(app.to_path_buf());
        assert_eq!(session.ui_messages().len(), 1);
        assert!(app.join("conversations").is_dir());
        assert!(app.join("transcript.jsonl.migrated").exists());
    }
}
