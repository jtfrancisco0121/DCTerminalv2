//! Codebase memory through `plugin/scripts/memory.sh` (docs/launch.md),
//! the only code that knows the memory layout. When the script is missing
//! (the plugin side is not there yet) everything degrades: no prompt file,
//! and the panel says memory is not available.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

pub const MEMORY_TIMEOUT: Duration = Duration::from_secs(30);

pub fn memory_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("memory")
}

pub fn script(plugin: &Path) -> Option<PathBuf> {
    let file = plugin.join("scripts").join("memory.sh");
    file.is_file().then_some(file)
}

/// `bash memory.sh <args>` with `DCV2_MEMORY_DIR`; stdout on success.
pub fn run_script(plugin: &Path, memory: &Path, args: &[&str]) -> Result<String, String> {
    let script = script(plugin).ok_or("memory.sh is not in this plugin yet")?;
    let mut cmd = Command::new("bash");
    cmd.arg(script)
        .args(args)
        .env("DCV2_MEMORY_DIR", memory)
        .env_remove("DCV2_TEST_MODE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_with_timeout(cmd, MEMORY_TIMEOUT)
}

/// Runs `cmd`, killing it (and its process group) after `timeout`.
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| format!("memory.sh: {e}"))?;
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            let _ = tx.send(buf);
        });
        rx
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as _));
    let err = read(child.stderr.take().map(|p| Box::new(p) as _));
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                #[cfg(unix)]
                {
                    let _ = Command::new("kill")
                        .args(["-9", &format!("-{}", child.id())])
                        .stderr(Stdio::null())
                        .status();
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err("memory.sh took too long".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("memory.sh: {e}")),
        }
    };
    let stdout = out.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let stderr = err.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let output = std::process::Output {
        status,
        stdout,
        stderr,
    };
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(err
            .lines()
            .next()
            .unwrap_or("memory.sh failed")
            .trim()
            .to_string())
    }
}

/// `memory.sh compose <folder> <out>`; None when it cannot (then the
/// launch goes without `--append-system-prompt-file`).
pub fn compose(plugin: &Path, memory: &Path, folder: &Path, out: &Path) -> Option<PathBuf> {
    let folder = folder.to_str()?;
    let out_str = out.to_str()?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    run_script(plugin, memory, &["compose", folder, out_str]).ok()?;
    out.is_file().then(|| out.to_path_buf())
}

/// `memory.sh status <folder>` as JSON, or `{"available": false, ...}`.
pub fn status(plugin: &Path, memory: &Path, folder: &Path) -> Value {
    let unavailable = |why: String| serde_json::json!({"available": false, "problem": why});
    let Some(folder) = folder.to_str() else {
        return unavailable("folder name is not text".into());
    };
    match run_script(plugin, memory, &["status", folder]) {
        Ok(out) => match serde_json::from_str::<Value>(&out) {
            Ok(Value::Object(mut map)) => {
                map.insert("available".into(), Value::Bool(true));
                Value::Object(map)
            }
            _ => unavailable("memory.sh status did not print JSON".into()),
        },
        Err(err) => unavailable(err),
    }
}

/// A memory file, read-only, only from inside the memory folder.
pub fn read_file(memory: &Path, rel: &str) -> Result<String, String> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute()
        || rel_path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("not a memory file".into());
    }
    let root = memory
        .canonicalize()
        .map_err(|_| "no memory yet".to_string())?;
    let path = root
        .join(rel_path)
        .canonicalize()
        .map_err(|_| "file not found".to_string())?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("not a memory file".into());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(text.chars().take(200_000).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn a_missing_script_degrades_and_files_stay_inside_the_memory_folder() {
        let dir = TempDir::new("memory");
        let plugin = dir.join("plugin");
        std::fs::create_dir_all(&plugin).unwrap();
        let memory = dir.join("memory");
        let status = status(&plugin, &memory, dir.path());
        assert_eq!(status["available"], false);
        assert!(compose(&plugin, &memory, dir.path(), &dir.join("out.md")).is_none());
        std::fs::create_dir_all(memory.join("repo1")).unwrap();
        std::fs::write(memory.join("repo1/index.md"), "# repo1\n").unwrap();
        std::fs::write(dir.join("secret.txt"), "no").unwrap();
        assert_eq!(read_file(&memory, "repo1/index.md").unwrap(), "# repo1\n");
        assert!(read_file(&memory, "../secret.txt").is_err());
        assert!(read_file(&memory, dir.join("secret.txt").to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_present_script_gives_status_json_and_a_prompt_file() {
        let dir = TempDir::new("memory_script");
        let plugin = dir.join("plugin");
        std::fs::create_dir_all(plugin.join("scripts")).unwrap();
        std::fs::write(
            plugin.join("scripts/memory.sh"),
            "case \"$1\" in\n status) printf '{\"schema\":1,\"root\":\"%s\",\"repos\":[]}' \"$DCV2_MEMORY_DIR\";;\n compose) echo note > \"$3\";;\n *) echo bad >&2; exit 2;;\nesac\n",
        )
        .unwrap();
        let memory = dir.join("memory");
        let status = status(&plugin, &memory, dir.path());
        assert_eq!(status["available"], true);
        assert_eq!(status["root"], memory.display().to_string());
        let out = compose(&plugin, &memory, dir.path(), &dir.join("p/out.md")).unwrap();
        assert_eq!(std::fs::read_to_string(out).unwrap(), "note\n");
        assert_eq!(run_script(&plugin, &memory, &["nope"]).unwrap_err(), "bad");
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 30"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = std::time::Instant::now();
        assert!(run_with_timeout(slow, Duration::from_millis(200))
            .unwrap_err()
            .contains("too long"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[test]
    fn the_prompt_file_is_replaced_not_written_through_a_planted_link() {
        let dir = TempDir::new("memory_prompt_link");
        let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugin");
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "outside").unwrap();
        let out = dir.join("run/memory-prompt.md");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &out).unwrap();
        let folder = dir.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let made = compose(&plugin, &dir.join("memory"), &folder, &out);
        assert!(made.is_some(), "the real memory.sh writes the prompt");
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside");
        assert!(!std::fs::symlink_metadata(&out)
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
