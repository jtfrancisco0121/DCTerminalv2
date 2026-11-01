// The gate's port of the Git Bash resolver (docs/launch.md, AD-2). The Rust one in
// src-tauri/src/platform/mod.rs and this one both run every case of
// fixtures/platform/git-bash-cases.json. Pure: env, the disk and `git --exec-path`
// are injected.
export const GIT_BASH_NOT_FOUND =
  "Git Bash not found. Install Git for Windows or set CLAUDE_CODE_GIT_BASH_PATH.";

const backslashes = (text) => String(text).replaceAll("/", "\\");

function rejected(candidate, systemRoot) {
  const lower = backslashes(candidate).toLowerCase();
  if (lower.includes("\\windowsapps\\")) return true;
  const system32 = systemRoot
    ? `${backslashes(systemRoot).toLowerCase()}\\system32\\`
    : "\\windows\\system32\\";
  return lower.startsWith(system32) || lower.includes("\\windows\\system32\\");
}

/**
 * The Git for Windows bash.exe, or throws the "not found" text. Order: the
 * CLAUDE_CODE_GIT_BASH_PATH file; %ProgramFiles%, %ProgramW6432%,
 * %ProgramFiles(x86)% and %LOCALAPPDATA%\Programs under Git\bin\bash.exe;
 * `git --exec-path` three folders up. System32 and WindowsApps are rejected.
 */
export function resolveGitBash(env, isFile, gitExecPath) {
  const systemRoot = env("SystemRoot");
  const usable = (candidate) =>
    !rejected(candidate, systemRoot) && isFile(candidate) ? candidate : null;
  const under = (name, tail) => {
    const base = env(name);
    return base ? `${base.replace(/[\\/]+$/, "")}\\${tail}` : null;
  };
  const tail = "Git\\bin\\bash.exe";
  const candidates = [
    env("CLAUDE_CODE_GIT_BASH_PATH") || null,
    under("ProgramFiles", tail),
    under("ProgramW6432", tail),
    under("ProgramFiles(x86)", tail),
    under("LOCALAPPDATA", `Programs\\${tail}`),
  ].filter(Boolean);
  for (const candidate of candidates) {
    const found = usable(candidate);
    if (found) return found;
  }
  const exec = gitExecPath();
  if (exec) {
    const parts = String(exec).split(/[\\/]+/).filter(Boolean);
    if (parts.length > 3) {
      const unc = /^[\\/]{2}/.test(exec);
      const base = `${unc ? "\\\\" : ""}${parts.slice(0, -3).join("\\")}`;
      const found = usable(`${base}\\bin\\bash.exe`);
      if (found) return found;
    }
  }
  throw new Error(GIT_BASH_NOT_FOUND);
}
