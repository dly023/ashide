// Ashide-bundled Omp extension.
//
// Bridges Omp's native extension events into Ashide's structured CLI agent
// protocol (OSC 777 `warp://cli-agent` sentinel). This gives Ashide:
//   - session_start → SessionStart (with session id + cwd)
//   - input → PromptSubmit (with the first real user prompt text)
//   - turn_end → Stop (best-effort completion signal)
//
// The extension is auto-installed into ~/.omp/agent/extensions/ashide-omp.ts
// by OmpPluginManager. Omp loads it via native extension auto-discovery.
//
// Protocol version 1; keep field names in sync with
// app/src/terminal/cli_agent_sessions/event/v1.rs.
//
// This file runs inside Omp's JS runtime, not Ashide's build. We use
// structural type guards instead of importing Omp's package types.

const SENTINEL = "warp://cli-agent";
const AGENT_NAME = "omp";

interface ExtensionContext {
  sessionManager?: {
    getSessionId(): string | null | undefined;
  };
}

function getSessionId(ctx: unknown): string | null {
  if (
    ctx &&
    typeof ctx === "object" &&
    "sessionManager" in ctx &&
    ctx.sessionManager &&
    typeof ctx.sessionManager === "object" &&
    typeof ctx.sessionManager.getSessionId === "function"
  ) {
    const id = ctx.sessionManager.getSessionId();
    return typeof id === "string" && id.length > 0 ? id : null;
  }
  return null;
}

function getEventText(event: unknown): string | null {
  if (!event || typeof event !== "object") return null;
  const candidate = "text" in event
    ? event.text
    : "input" in event
      ? event.input
      : "message" in event
        ? event.message
        : undefined;
  return typeof candidate === "string" && candidate.trim().length > 0
    ? candidate
    : null;
}

function emit(event: string, payload: Record<string, unknown>): void {
  const body = JSON.stringify({ v: 1, agent: AGENT_NAME, event, ...payload });
  // OSC 777 ; notify ; <title> ; <body> ST
  process.stdout.write(`\x1b]777;notify;${SENTINEL};${body}\x07`);
}

export default function (pi: {
  on(event: string, handler: (event: unknown, ctx: unknown) => void | Promise<void>): void;
}): void {
  pi.on("session_start", async (_event: unknown, ctx: unknown) => {
    const sessionId = getSessionId(ctx);
    const cwd = process.cwd();
    emit("session_start", {
      session_id: sessionId,
      cwd,
      plugin_version: "1.0.0",
    });
  });

  pi.on("input", async (event: unknown) => {
    const text = getEventText(event);
    if (text === null) return;
    // Skip slash commands — they're not user prompts.
    if (text.startsWith("/")) return;
    const sessionId = getSessionId(event);
    emit("prompt_submit", {
      session_id: sessionId,
      query: text,
      cwd: process.cwd(),
    });
  });

  pi.on("turn_end", async () => {
    emit("stop", {});
  });
}
