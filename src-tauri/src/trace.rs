// 作者: Corin-Sands
//! 本机串联日志：一次用户动作一个 traceId，落盘 `trace.log`，失败文案附带同一 id。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{FixedOffset, SecondsFormat};
use uuid::Uuid;

/// 一次动作的追踪上下文（对话一轮 / 一次 MCP tools/call）。
/// Author: Corin-Sands
#[derive(Clone)]
pub struct Trace {
    pub id: String,
    root: PathBuf,
}

impl Trace {
    pub fn start(root: impl AsRef<Path>, kind: &str) -> Self {
        let id = format!("tr_{}", &Uuid::new_v4().simple().to_string()[..12]);
        let trace = Self {
            id,
            root: root.as_ref().to_path_buf(),
        };
        trace.event("start", kind);
        trace
    }

    pub fn event(&self, stage: &str, detail: &str) {
        let line = format!(
            "{} traceId={} stage={} {}\n",
            now_stamp(),
            self.id,
            stage,
            sanitize(detail)
        );
        let path = self.root.join("trace.log");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(line.as_bytes());
        }
        eprint!("[trace] {}", line);
    }

    /// 失败文案末尾挂 traceId，便于 UI 展示与日志对账。
    pub fn annotate(&self, err: impl AsRef<str>) -> String {
        let msg = err.as_ref().trim();
        if msg.contains("traceId=") {
            msg.to_string()
        } else {
            format!("{msg} [traceId={}]", self.id)
        }
    }

    pub fn fail(&self, stage: &str, err: impl AsRef<str>) -> String {
        let msg = err.as_ref().trim();
        self.event(stage, &format!("error={msg}"));
        self.annotate(msg)
    }
}

fn now_stamp() -> String {
    let offset = FixedOffset::east_opt(8 * 3600).unwrap_or(FixedOffset::east_opt(0).unwrap());
    chrono::Utc::now()
        .with_timezone(&offset)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn sanitize(detail: &str) -> String {
    detail
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .take(500)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn annotate_appends_trace_id_once() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Trace::start(dir.path(), "test");
        let once = trace.annotate("模型请求失败: boom");
        assert!(once.contains(&trace.id));
        assert!(once.contains("traceId="));
        let twice = trace.annotate(&once);
        assert_eq!(twice.matches("traceId=").count(), 1);
    }

    #[test]
    fn event_writes_trace_log() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Trace::start(dir.path(), "unit");
        trace.event("llm.request", "url=https://example/v1/chat/completions");
        let text = fs::read_to_string(dir.path().join("trace.log")).unwrap();
        assert!(text.contains(&trace.id));
        assert!(text.contains("stage=llm.request"));
        assert!(text.lines().count() >= 2);
    }
}
