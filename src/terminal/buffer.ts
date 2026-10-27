/** Holds PTY bytes until an xterm instance attaches. */

/**
 * Most bytes held while nothing is attached: about the 10000-line scrollback
 * xterm keeps anyway. Older bytes are dropped first.
 */
export const MAX_PENDING_BYTES = 2 * 1024 * 1024;

export type PtyBuffer = {
  pushData: (bytes: Uint8Array) => void;
  pushExit: (code: number | null) => void;
  attach: (handlers: {
    data: (bytes: Uint8Array) => void;
    exit: (code: number | null) => void;
  }) => () => void;
};

export function createPtyBuffer(): PtyBuffer {
  let pending: Uint8Array[] = [];
  let pendingBytes = 0;
  let exitCode: number | null | undefined;
  let handlers: {
    data: (bytes: Uint8Array) => void;
    exit: (code: number | null) => void;
  } | null = null;

  /** Keeps the newest `MAX_PENDING_BYTES`; the oldest kept chunk may be cut. */
  function trimPending(): void {
    let excess = pendingBytes - MAX_PENDING_BYTES;
    if (excess <= 0) return;
    let drop = 0;
    while (drop < pending.length && excess > 0) {
      const chunk = pending[drop];
      if (chunk.length <= excess) {
        excess -= chunk.length;
        pendingBytes -= chunk.length;
        drop += 1;
      } else {
        pending[drop] = chunk.subarray(excess);
        pendingBytes -= excess;
        excess = 0;
      }
    }
    pending = pending.slice(drop);
  }

  return {
    pushData(bytes) {
      if (handlers) handlers.data(bytes);
      else {
        pending.push(bytes);
        pendingBytes += bytes.length;
        trimPending();
      }
    },
    pushExit(code) {
      exitCode = code;
      handlers?.exit(code);
    },
    attach(next) {
      handlers = next;
      for (const chunk of pending) next.data(chunk);
      pending = [];
      pendingBytes = 0;
      if (exitCode !== undefined) next.exit(exitCode);
      return () => {
        if (handlers === next) handlers = null;
      };
    },
  };
}

export function decodeBase64(data: string): Uint8Array {
  if (!data) return new Uint8Array();
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}
