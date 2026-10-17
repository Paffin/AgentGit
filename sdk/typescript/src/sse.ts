import { AgentGitError, errorFromInfo, isAgentGitError } from './errors.js';
import { OPERATIONS } from './generated/operations.js';
import type { AgentEvent, ReconnectOptions, SubscribeEventsOptions } from './types.js';
import type { Transport } from './transport.js';

/** One dispatched Server-Sent Event. */
export interface SseFrame {
  /** `event:` field (`message` when absent). */
  event: string;
  data: string;
  /** Last `id:` seen on the connection. */
  id: string;
  /** `retry:` hint in milliseconds, when the server sent one. */
  retry?: number;
}

class IdleTimeout extends Error {
  constructor(ms: number) {
    super(`no data (not even a heartbeat) for ${ms} ms`);
    this.name = 'IdleTimeout';
  }
}

/**
 * Parses a Server-Sent Events byte stream. Comments (heartbeats) produce no frame but count as
 * activity for the idle watchdog. Handles `\n`, `\r\n` and `\r` line ends and a leading BOM.
 */
export async function* readSse(
  body: ReadableStream<Uint8Array>,
  idleTimeoutMs = 0,
): AsyncGenerator<SseFrame, void, undefined> {
  const reader = body.getReader();
  const decoder = new TextDecoder('utf-8');
  let buffer = '';
  let first = true;
  let event = '';
  let data: string[] = [];
  let lastId = '';
  let retry: number | undefined;
  try {
    for (;;) {
      let chunk: ReadableStreamReadResult<Uint8Array>;
      if (idleTimeoutMs > 0) {
        let timer: ReturnType<typeof setTimeout> | undefined;
        const idle = new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(new IdleTimeout(idleTimeoutMs)), idleTimeoutMs);
        });
        try {
          chunk = await Promise.race([reader.read(), idle]);
        } finally {
          clearTimeout(timer);
        }
      } else {
        chunk = await reader.read();
      }
      if (chunk.done) {
        buffer += decoder.decode();
      } else {
        buffer += decoder.decode(chunk.value, { stream: true });
      }
      if (first && buffer.length > 0) {
        if (buffer.charCodeAt(0) === 0xfeff) buffer = buffer.slice(1);
        first = false;
      }
      // A trailing "\r" might be the first half of "\r\n": wait for more input unless the stream ended.
      const lines: string[] = [];
      let start = 0;
      for (let i = 0; i < buffer.length; i++) {
        const c = buffer[i];
        if (c === '\n') {
          lines.push(buffer.slice(start, i));
          start = i + 1;
        } else if (c === '\r') {
          if (i + 1 === buffer.length && !chunk.done) break;
          lines.push(buffer.slice(start, i));
          start = buffer[i + 1] === '\n' ? i + 2 : i + 1;
          if (buffer[i + 1] === '\n') i++;
        }
      }
      buffer = buffer.slice(start);
      if (chunk.done && buffer.length > 0) {
        lines.push(buffer);
        buffer = '';
      }
      for (const line of lines) {
        if (line === '') {
          if (data.length > 0) {
            const frame: SseFrame = { event: event || 'message', data: data.join('\n'), id: lastId };
            if (retry !== undefined) frame.retry = retry;
            yield frame;
          }
          event = '';
          data = [];
          retry = undefined;
          continue;
        }
        if (line.startsWith(':')) continue;
        const colon = line.indexOf(':');
        const field = colon === -1 ? line : line.slice(0, colon);
        let value = colon === -1 ? '' : line.slice(colon + 1);
        if (value.startsWith(' ')) value = value.slice(1);
        if (field === 'event') event = value;
        else if (field === 'data') data.push(value);
        else if (field === 'id' && !value.includes('\0')) lastId = value;
        else if (field === 'retry' && /^\d+$/.test(value)) retry = Number(value);
      }
      if (chunk.done) return;
    }
  } finally {
    reader.cancel().catch(() => undefined);
  }
}

const DEFAULT_IDLE_MS = 90_000;
const DEFAULT_RECONNECT: Required<ReconnectOptions> = {
  maxAttempts: Number.POSITIVE_INFINITY,
  baseDelayMs: 200,
  maxDelayMs: 10_000,
};

/**
 * Follows a repository's event feed as an async iterator. Uses `fetch` streaming, so the
 * `Authorization` header works (browser `EventSource` cannot send it). When the connection drops or
 * the server closes the stream, it reconnects and resumes after the last delivered `seq`; events are
 * never delivered twice and heartbeats are never yielded.
 */
export async function* subscribeEvents(
  transport: Transport,
  options: SubscribeEventsOptions,
): AsyncGenerator<AgentEvent, void, undefined> {
  const op = OPERATIONS['events.subscribe'];
  const { signal: outer } = options;
  if (outer?.aborted) return;

  const reconnect = options.reconnect === false ? undefined : { ...DEFAULT_RECONNECT, ...(options.reconnect ?? {}) };
  const idleMs = options.idleTimeoutMs ?? DEFAULT_IDLE_MS;
  const controller = new AbortController();
  const onAbort = (): void => controller.abort();
  outer?.addEventListener('abort', onAbort, { once: true });
  const { sleep, random } = transport.config;

  try {
    let lastSeq = options.afterSeq;
    if (lastSeq === undefined) {
      // "From now": pin down the current end of the log, so that a reconnect cannot skip events
      // that were appended between two connections.
      const probe = (await transport.execute({
        op: OPERATIONS['events.replay'],
        request: { repo: options.repo, after_seq: Number.MAX_SAFE_INTEGER, limit: 1 },
        data: undefined,
        options: { ...(outer ? { signal: outer } : {}), ...(options.runId ? { runId: options.runId } : {}) },
      })) as { latest_seq?: number };
      lastSeq = probe.latest_seq ?? 0;
    }

    let failures = 0;
    for (;;) {
      if (controller.signal.aborted) return;
      let delivered = false;
      let connectedAt = 0;
      try {
        const request: Record<string, unknown> = { repo: options.repo, after_seq: lastSeq };
        if (options.topics && options.topics.length > 0) request['topics'] = options.topics;
        if (options.heartbeatMs !== undefined) request['heartbeat_ms'] = options.heartbeatMs;
        const response = await transport.openStream(op, request, {
          signal: controller.signal,
          runId: options.runId,
          headers: options.headers,
        });
        connectedAt = Date.now();
        if (!response.body)
          throw new AgentGitError({
            code: 'unavailable',
            message: 'the event stream has no body',
            retryable: true,
          });
        for await (const frame of readSse(response.body, idleMs)) {
          if (frame.event === 'error') {
            const info = (JSON.parse(frame.data) as { error?: Record<string, unknown> }).error ?? {};
            throw errorFromInfo(info, { status: 0, operation: op.id });
          }
          const event = JSON.parse(frame.data) as AgentEvent;
          if (typeof event.seq !== 'number' || event.seq <= lastSeq) continue; // already delivered
          lastSeq = event.seq;
          delivered = true;
          failures = 0;
          yield event;
        }
      } catch (e) {
        if (controller.signal.aborted) return;
        // A dropped connection surfaces as a TypeError from the fetch body stream; a malformed event
        // (SyntaxError) is a protocol problem and is not retried.
        const transient = e instanceof IdleTimeout || (isAgentGitError(e) && e.retryable) || e instanceof TypeError;
        if (!reconnect || !transient) throw e;
      }
      if (!reconnect) return;
      if (connectedAt > 0 && Date.now() - connectedAt > 5000) failures = 0;
      if (!delivered) failures++;
      if (failures > reconnect.maxAttempts) {
        throw new AgentGitError({
          code: 'unavailable',
          message: `the event stream could not be re-established after ${reconnect.maxAttempts} attempts`,
          retryable: true,
        });
      }
      const ceiling = Math.min(reconnect.maxDelayMs, reconnect.baseDelayMs * 2 ** Math.max(0, failures - 1));
      try {
        await sleep(delivered ? 0 : Math.round(random() * ceiling), controller.signal);
      } catch {
        return;
      }
    }
  } finally {
    outer?.removeEventListener('abort', onAbort);
    controller.abort(); // closes the connection when the consumer stops early
  }
}
