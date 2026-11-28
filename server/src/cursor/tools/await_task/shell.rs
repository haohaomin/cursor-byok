//! Observe terminal files through the client that owns the workspace.
use super::{output, receive, Arguments, ExecContext, Response, Wait};
use crate::{cursor::protocol::proto::agent::v1 as pb, Error, Result};
use regex::Regex;
use std::{sync::OnceLock, time::Duration};
use tokio::time::Instant;

pub(super) struct Shell {
    pub path: String,
    pattern: Option<Regex>,
}

impl Shell {
    pub fn new(args: &Arguments, context: &ExecContext) -> Result<Self> {
        if context.terminals_folder.is_empty() {
            return Err(Error::Protocol(
                "Await requires the client terminals folder".into(),
            ));
        }
        let pattern = args
            .pattern
            .as_ref()
            .filter(|p| !p.is_empty())
            .map(|pattern| {
                regex::RegexBuilder::new(pattern)
                    .multi_line(true)
                    .build()
                    .map_err(|e| Error::Protocol(format!("Invalid Await pattern: {e}")))
            })
            .transpose()?;
        Ok(Self {
            path: format!(
                "{}/{}.txt",
                context.terminals_folder.trim_end_matches(['/', '\\']),
                args.task_id
            ),
            pattern,
        })
    }

    pub async fn run(
        self,
        wait: &Wait,
        mut response: Response,
    ) -> std::result::Result<pb::await_success::AwaitResult, String> {
        let mut running_snapshot = None::<(u64, Instant)>;
        loop {
            let (text, length) = read_output(receive(response).await?)?;
            let observed = terminal_output(&text);
            if let Some(age) = observed.running_ms {
                if running_snapshot.is_none_or(|(previous, _)| previous != age) {
                    running_snapshot = Some((age, Instant::now()));
                }
            }
            let elapsed = observed
                .runtime_ms
                .or_else(|| {
                    if observed.completed {
                        return observed.running_ms;
                    }
                    running_snapshot.map(|(age, read_at)| {
                        let projected = age.saturating_add(read_at.elapsed().as_millis() as u64);
                        let Some(start) = observed.started_at_ms else {
                            return projected;
                        };
                        let clock_age = (i128::from(chrono::Utc::now().timestamp_millis())
                            - i128::from(start))
                        .max(0);
                        let drift = clock_age - i128::from(projected);
                        // Client and server clocks may differ. Only use the wall clock
                        // within the same tolerance as the native runtime.
                        if (-1000..=6000).contains(&drift) {
                            projected.max(clock_age as u64)
                        } else {
                            projected
                        }
                    })
                })
                .unwrap_or(wait.started.elapsed().as_millis() as u64);
            let found = self
                .pattern
                .as_ref()
                .and_then(|pattern| pattern.find(observed.body))
                .filter(|m| !m.is_empty())
                .map(|m| bounded_match(m.as_str()));
            if observed.completed {
                return Ok(output::complete(
                    &wait.args.task_id,
                    elapsed,
                    &self.path,
                    length,
                    self.pattern.is_some(),
                    found,
                    observed.exit_code,
                ));
            }
            if found.is_some()
                || wait.started.elapsed() >= Duration::from_millis(u64::from(wait.args.wait_ms))
            {
                return Ok(output::running(
                    &wait.args.task_id,
                    elapsed,
                    &self.path,
                    length,
                    self.pattern.is_some(),
                    found,
                ));
            }
            let remaining = Duration::from_millis(u64::from(wait.args.wait_ms))
                .saturating_sub(wait.started.elapsed());
            tokio::time::sleep(remaining.min(Duration::from_millis(250))).await;
            let (message, next) = wait.read(&self.path).await.map_err(|e| e.to_string())?;
            wait.queue(message).await;
            response = next;
        }
    }
}

pub(super) fn read_output(
    message: pb::exec_client_message::Message,
) -> std::result::Result<(String, u64), String> {
    let pb::exec_client_message::Message::ReadResult(result) = message else {
        return Err("Expected ReadResult from Cursor".into());
    };
    use pb::read_result::Result;
    match result.result {
        Some(Result::Success(value)) => {
            if value.truncated || value.range_applied {
                return Err(
                    "Terminal output was truncated; cannot determine the complete task state"
                        .into(),
                );
            }
            let text = match value.output {
                Some(pb::read_success::Output::Content(text)) => text,
                Some(pb::read_success::Output::Data(data)) => String::from_utf8(data)
                    .map_err(|_| "Terminal output is not UTF-8".to_string())?,
                None => return Err("Cursor did not return terminal file contents".into()),
            };
            let length = u64::try_from(value.file_size)
                .ok()
                .filter(|n| *n > 0)
                .unwrap_or_else(|| text.encode_utf16().count() as u64);
            Ok((text, length))
        }
        Some(Result::FileNotFound(_)) => Err("No shell found for the requested task_id".into()),
        Some(Result::Rejected(value)) => Err(format!("Terminal read rejected: {}", value.reason)),
        Some(Result::PermissionDenied(_)) => Err("Terminal read permission denied".into()),
        Some(Result::Error(value)) => Err(value.error),
        _ => Err("Cursor could not read the terminal output".into()),
    }
}

struct Terminal<'a> {
    body: &'a str,
    completed: bool,
    exit_code: Option<i32>,
    runtime_ms: Option<u64>,
    running_ms: Option<u64>,
    started_at_ms: Option<i64>,
}

fn terminal_output(text: &str) -> Terminal<'_> {
    let mut state = Terminal {
        body: text,
        completed: false,
        exit_code: None,
        runtime_ms: None,
        running_ms: None,
        started_at_ms: None,
    };
    static HEADER: OnceLock<Regex> = OnceLock::new();
    let header = HEADER.get_or_init(|| {
        Regex::new(r"(?s)\A---\r?\n(.*?)\r?\n---(?:\r?\n|\z)").expect("valid terminal header regex")
    });
    if let Some(capture) = header.captures(text) {
        for line in capture[1].lines() {
            if let Some(age) = line.strip_prefix("running_for_ms:") {
                state.running_ms = age
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|age| *age <= 9_007_199_254_740_991);
            }
            if let Some(start) = line.strip_prefix("started_at:") {
                state.started_at_ms = chrono::DateTime::parse_from_rfc3339(start.trim())
                    .ok()
                    .map(|date| date.timestamp_millis());
            }
        }
    }
    // Parse completion on the original file. Stripping its header first can
    // consume the opening newline of a footer for a task with no stdout.
    let footer = text
        .trim_end()
        .strip_suffix("\n---")
        .and_then(|text| text.rsplit_once("\n---\n"));
    if let Some((_, footer)) = footer {
        for line in footer.lines() {
            if let Some(code) = line.strip_prefix("exit_code:") {
                state.completed = true;
                state.exit_code = code.trim().parse().ok();
            }
            if let Some(ms) = line.strip_prefix("elapsed_ms:") {
                state.runtime_ms = ms.trim().parse().ok();
            }
        }
        if !state.completed {
            state.runtime_ms = None;
        }
    }
    if let Some(header) = text.strip_prefix("---\n") {
        if let Some((_, output)) = header.split_once("\n---\n") {
            state.body = output;
        }
    }
    if let Some((prefix, _)) = footer {
        let body_start = text.len() - state.body.len();
        state.body = text.get(body_start..prefix.len()).unwrap_or_default();
    }
    // Without a footer, trailing whitespace belongs to the process output.
    state
}

fn bounded_match(value: &str) -> String {
    if value.chars().take(1001).count() <= 1000 {
        return value.into();
    }
    let head = value.chars().take(500).collect::<String>();
    let mut tail = value.chars().rev().take(499).collect::<Vec<_>>();
    tail.reverse();
    format!("{head}…{}", tail.into_iter().collect::<String>())
}
