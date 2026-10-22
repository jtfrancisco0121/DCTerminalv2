/** Holds PTY bytes until an xterm instance attaches. */

export type PtyBuffer = {
  pushData: (bytes: Uint8Array) => void;
  pushExit: (code: number | null) => void;
  attach: (handlers: {
    data: (bytes: Uint8Array) => void;
    exit: (code: number | null) => void;
  }) => () => void;
};

export function createPtyBuffer(): PtyBuffer {
  const pending: Uint8Array[] = [];
  let exitCode: number | null | undefined;
  let handlers: {
    data: (bytes: Uint8Array) => void;
    exit: (code: number | null) => void;
  } | null = null;

  return {
    pushData(bytes) {
      if (handlers) handlers.data(bytes);
      else pending.push(bytes);
    },
    pushExit(code) {
      exitCode = code;
      handlers?.exit(code);
    },
    attach(next) {
      handlers = next;
      for (const chunk of pending) next.data(chunk);
      pending.length = 0;
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
