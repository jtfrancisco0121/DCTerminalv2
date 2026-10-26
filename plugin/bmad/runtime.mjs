// DCTerminalv2 wrapper around the vendored BMAD runtime (ade-runtime.mjs).
//
//   DCV2_RUN_DIR=<run folder> node runtime.mjs <command> <args...>
//
// What it guarantees (the BMAD skills are text the model follows, so this is the
// hard edge):
//   - only the commands the vendored skills use are allowed (see COMMANDS);
//   - DCV2_RUN_DIR is required; the runtime's project root is always <run>/bmad
//     (added when missing, refused when it points anywhere else);
//   - every --workspace / --path / --output must be an absolute path that, after
//     resolving symlinks, is inside <run>/bmad (the nearest existing parent is
//     resolved for paths that do not exist yet, a dangling symlink is refused);
//   - the filesystem handed to the runtime reads anywhere, but writes, appends,
//     mkdir and delete are refused unless the real path is inside <run>/bmad.
// Output and exit codes follow ADE's own CLI: stdout of the command, exit 0/1/2.
import { promises as fsp } from "node:fs";
import { isAbsolute, join, resolve, sep, dirname } from "node:path";
import { pathToFileURL } from "node:url";

/** The runtime commands the vendored skills call (keep in sync with scripts/patch-bmad.mjs). */
export const COMMANDS = ["memlog", "resolve_config", "resolve_customization", "knowledge", "lint_spine"];
/** Commands that take --project-root (a missing one is added). */
const TAKES_PROJECT_ROOT = new Set(["resolve_config", "resolve_customization"]);
/** Flags whose value is a path the command may write under. */
const WRITE_PATH_FLAGS = new Set(["--workspace", "--path", "--output", "-o"]);

class Refusal extends Error {}

const utf8 = new TextDecoder("utf-8", { fatal: true });

/** Real path of `p`, or of its nearest existing parent plus the missing tail. Fails closed. */
export async function realish(p) {
  let cur = resolve(p);
  const tail = [];
  for (;;) {
    try {
      const real = await fsp.realpath(cur);
      return tail.length ? join(real, ...tail.reverse()) : real;
    } catch (e) {
      if (e && e.code !== "ENOENT" && e.code !== "ENOTDIR") throw new Refusal(`cannot resolve ${p}: ${e.code ?? e}`);
    }
    // realpath failed with ENOENT: a dangling symlink also lands here, refuse it.
    try {
      await fsp.lstat(cur);
      throw new Refusal(`dangling or unresolvable link: ${cur}`);
    } catch (e) {
      if (e instanceof Refusal) throw e;
      if (e && e.code !== "ENOENT" && e.code !== "ENOTDIR") throw new Refusal(`cannot resolve ${p}: ${e.code ?? e}`);
    }
    const up = dirname(cur);
    if (up === cur) throw new Refusal(`cannot resolve ${p}`);
    tail.push(cur.slice(up.length === 1 ? 1 : up.length + 1));
    cur = up;
  }
}

function inside(root, real) {
  return real === root || real.startsWith(root.endsWith(sep) ? root : root + sep);
}

/** Build the guard for a run folder. */
export async function makeGuard(runDir) {
  if (!runDir || !isAbsolute(runDir)) throw new Refusal("DCV2_RUN_DIR must be set to the absolute run folder");
  const realRun = await fsp.realpath(runDir).catch(() => {
    throw new Refusal(`DCV2_RUN_DIR does not exist: ${runDir}`);
  });
  const root = join(realRun, "bmad");
  const check = async (p, what) => {
    if (typeof p !== "string" || !isAbsolute(p)) throw new Refusal(`${what} must be an absolute path: ${p}`);
    const real = await realish(p);
    if (!inside(root, real)) throw new Refusal(`${what} is outside the run's bmad folder: ${p}`);
    return real;
  };
  return { root, check };
}

/** An Fs for the runtime: reads anywhere, writes only inside the guard's root. */
export function guardedFs(guard) {
  const w = async (p, what) => {
    await guard.check(p, `write to ${what}`);
  };
  return {
    readText: async (p) => utf8.decode(await fsp.readFile(p)),
    writeText: async (p, body) => {
      await w(p, p);
      await fsp.writeFile(p, body);
    },
    append: async (p, body) => {
      await w(p, p);
      await fsp.appendFile(p, body);
    },
    list: (p) => fsp.readdir(p),
    exists: (p) => fsp.access(p).then(() => true, () => false),
    mkdir: async (p) => {
      await w(p, p);
      await fsp.mkdir(p, { recursive: true });
    },
    delete: async (p) => {
      await w(p, p);
      await fsp.unlink(p);
    },
    isSymlink: (p) => fsp.lstat(p).then((st) => st.isSymbolicLink(), () => false),
  };
}

/** Validate argv and return the argv to pass on. Throws Refusal. */
export async function checkArgs(guard, argv) {
  const [command, ...rest] = argv;
  if (command === undefined) throw Object.assign(new Refusal("usage: node runtime.mjs <command> <args...>"), { exitCode: 2 });
  if (!COMMANDS.includes(command)) {
    throw Object.assign(new Refusal(`command not allowed: ${command} (allowed: ${COMMANDS.join(", ")})`), { exitCode: 2 });
  }
  let sawProjectRoot = false;
  for (let i = 0; i < rest.length; i++) {
    const token = rest[i];
    const cut = token.startsWith("--") ? token.indexOf("=") : -1;
    const flag = cut === -1 ? token : token.slice(0, cut);
    const take = () => {
      if (cut !== -1) return token.slice(cut + 1);
      return rest[++i];
    };
    if (flag === "--project-root") {
      sawProjectRoot = true;
      const v = take();
      if (typeof v !== "string" || !isAbsolute(v)) throw new Refusal(`--project-root must be an absolute path: ${v}`);
      const real = await realish(v);
      if (real !== guard.root) throw new Refusal(`--project-root must be the run's bmad folder (${guard.root}), got: ${v}`);
    } else if (WRITE_PATH_FLAGS.has(flag)) {
      const v = take();
      await guard.check(v, flag);
    }
  }
  const out = [command, ...rest];
  if (!sawProjectRoot && TAKES_PROJECT_ROOT.has(command)) out.push("--project-root", guard.root);
  return out;
}

export async function main(argv, env = process.env) {
  try {
    const guard = await makeGuard(env.DCV2_RUN_DIR);
    const args = await checkArgs(guard, argv);
    const { cliMain } = await import("./ade-runtime.mjs");
    const r = await cliMain(args, guardedFs(guard));
    if (r.stdout) process.stdout.write(r.stdout.endsWith("\n") ? r.stdout : `${r.stdout}\n`);
    return r.exitCode;
  } catch (error) {
    if (error instanceof Refusal) {
      process.stderr.write(`bmad runtime: refused: ${error.message}\n`);
      return error.exitCode ?? 1;
    }
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return 1;
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  process.exitCode = await main(process.argv.slice(2));
}
