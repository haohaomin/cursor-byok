//! Plan document policy. Resolve client paths lexically, never against the server filesystem.
use percent_encoding::percent_decode_str;
use std::sync::LazyLock;
use typed_path::Utf8TypedPath;
use url::Url;

use crate::cursor::protocol::proto::agent::v1 as pb;
use crate::cursor::tools::text::js_whitespace;

#[derive(Clone, Debug, Default)]
pub struct PlanEdits {
    windows: Option<bool>,
    cwd: Option<String>,
    home: Option<String>,
}

impl PlanEdits {
    pub(crate) fn new(env: Option<&pb::RequestContextEnv>) -> Self {
        let Some(env) = env else {
            return Self::default();
        };
        let cwd = env
            .process_working_directory
            .clone()
            .filter(|p| !p.is_empty());
        let os = env.os_version.to_ascii_lowercase();
        let windows = if os.contains("windows") || os.starts_with("win32") {
            Some(true)
        } else if os.contains("linux") || os.contains("darwin") || os.contains("mac") {
            Some(false)
        } else {
            cwd.as_deref()
                .map(|p| matches!(Utf8TypedPath::derive(p), Utf8TypedPath::Windows(_)))
        };
        Self {
            windows,
            home: (env.is_working_dir_home_dir == Some(true))
                .then(|| cwd.clone())
                .flatten(),
            cwd,
        }
    }

    pub(crate) fn validate(&self, input: &str) -> Result<(), String> {
        let path = self.resolve(input)?;
        let path = path.trim_matches(js_whitespace).to_lowercase();
        if [".md", ".markdown", ".mdown", ".mkd", ".mkdn", ".mdx"]
            .iter()
            .any(|suffix| path.ends_with(suffix))
        {
            return Ok(());
        }
        static CANVAS: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(r"(?:^|/)\.cursor/projects/[^/]+/canvases/[^/]+\.canvas\.tsx$")
                .unwrap()
        });
        if CANVAS.is_match(&path.replace('\\', "/")) {
            return Ok(());
        }
        Err("Plan mode can only edit Markdown documents (.md, .markdown, .mdown, .mkd, .mkdn, .mdx) and .cursor/projects/<project>/canvases/<name>.canvas.tsx. Wait for the user to approve implementation before editing other files.".into())
    }

    fn resolve(&self, input: &str) -> Result<String, String> {
        let mut path = if input.starts_with("file://") {
            file_path(input, self.windows)?
        } else {
            input.to_owned()
        };
        if path.is_empty() || path.contains('\0') {
            return Err("Plan edit requires a nonempty file path without NUL characters.".into());
        }
        if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
            let home = self.home.as_deref().ok_or_else(|| {
                "The client's home directory is unknown. Use the plan's absolute file path instead of ~/.".to_string()
            })?;
            path = format!("{home}{}", &path[1..]);
        }
        let windows = self
            .windows
            .unwrap_or_else(|| matches!(Utf8TypedPath::derive(&path), Utf8TypedPath::Windows(_)));
        let typed = if windows {
            Utf8TypedPath::windows(&path)
        } else {
            Utf8TypedPath::unix(&path)
        };
        let resolved = if typed.is_absolute() {
            typed.normalize()
        } else {
            let cwd = self.cwd.as_deref().ok_or_else(|| {
                "The client's working directory is unknown. Use an absolute file path for the Plan edit.".to_string()
            })?;
            let base = if windows {
                Utf8TypedPath::windows(cwd)
            } else {
                Utf8TypedPath::unix(cwd)
            };
            if !base.is_absolute() {
                return Err("The client did not provide an absolute working directory. Use an absolute file path for the Plan edit.".into());
            }
            // Windows' C:foo uses the current directory of C:, not its root.
            // Only the client's current drive is known; never read the server's
            // per-drive environment to resolve another drive.
            let relative = if windows
                && path.as_bytes().get(1) == Some(&b':')
                && path.as_bytes()[0].is_ascii_alphabetic()
                && cwd.as_bytes().get(1) == Some(&b':')
                && path.as_bytes()[0].eq_ignore_ascii_case(&cwd.as_bytes()[0])
            {
                &path[2..]
            } else {
                &path
            };
            let joined = base.join(relative);
            if !joined.is_absolute() {
                return Err("This drive-relative path needs another client working directory. Use a fully qualified absolute file path.".into());
            }
            joined.normalize()
        };
        if resolved.file_name().is_none() {
            return Err(
                "Plan edit requires a file, not a filesystem root or network share.".into(),
            );
        }
        Ok(resolved.as_str().to_owned())
    }
}

// Decode file URIs without url::to_file_path(), whose Windows behavior depends
// on the server's build target. Exec still receives the original client path.
fn file_path(input: &str, windows: Option<bool>) -> Result<String, String> {
    let invalid = || "Invalid file URI. Use the plan's absolute file path.".to_string();
    let url = Url::parse(input).map_err(|_| invalid())?;
    let raw = url.path();
    let host = url.host_str().filter(|host| *host != "localhost");
    let bytes = raw.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'%'
            && (i + 2 >= bytes.len() || !bytes[i + 1..i + 3].iter().all(u8::is_ascii_hexdigit))
        {
            return Err(invalid());
        }
    }
    let decoded = percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| invalid())?;
    let drive = decoded.as_bytes().get(2) == Some(&b':')
        && decoded
            .as_bytes()
            .get(1)
            .is_some_and(u8::is_ascii_alphabetic);
    let windows = windows.unwrap_or(drive || host.is_some());
    let lower = raw.to_ascii_lowercase();
    if lower.contains("%2f") || (windows && lower.contains("%5c")) {
        return Err(invalid());
    }
    if let Some(host) = host {
        return if windows {
            Ok(format!("\\\\{host}{}", decoded.replace('/', "\\")))
        } else {
            Err(invalid())
        };
    }
    // file:///C:/... is a Windows path even on a Linux/macOS BYOK server.
    if windows && drive {
        Ok(decoded[1..].to_owned())
    } else if windows {
        Err(invalid())
    } else {
        Ok(decoded.into_owned())
    }
}

#[cfg(test)]
mod tests;
