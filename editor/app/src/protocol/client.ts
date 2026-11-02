// TS client for the ludion wire protocol (engine-architecture.md §6).
//
// Handshake: send `Hello` as the first text message, validate `ServerHello.protocol_version`; an
// `error` envelope in reply surfaces as the engine's own `ProtocolError`, and `connect` always settles
// (close or timeout rejects). The schema hash is not checked here — that is the caller's (`schemaCheck`).
// After that, `call()` sends a `Request` and resolves/rejects by matching the response `id`
// (responses may interleave with events — hence a map keyed by id, not a queue). Incoming text
// messages are demuxed into `Response` vs `EventMessage` by shape (`classifyTextMessage`, kept
// pure and exported so it's testable without a real socket); incoming binary messages are `LFR1`
// frames, decoded by `./frames`.
//
// No auto-reconnect: a dropped connection rejects every pending call and notifies `onClose`
// listeners. The caller owns reconnection (`viewer/reconnect.ts`, driven by `App.tsx`).

import type {
  Command,
  EventMessage,
  Hello,
  Request,
  Response,
  ResponseBody,
  ServerHello,
  StreamSubscribeParams,
  WireErrorSchema,
} from '@ludion/protocol';
import { decodeFrameHeader, type DecodedFrame } from './frames';
import { parseWire, stringifyWire } from './wireJson';

export const PROTOCOL_VERSION = 1;
export const CLIENT_NAME = 'ludion-viewer';
/** How long `connect` waits for the `ServerHello` before giving up. */
export const CONNECT_TIMEOUT_MS = 10_000;

export type Channel = StreamSubscribeParams['channel'];
export type SubscribeMode = NonNullable<StreamSubscribeParams['mode']>;
/** The wire's own codec names, taken from the generated params rather than restated, so a format
 *  added to the protocol reaches this client by regenerating the types. */
export type SubscribeFormat = NonNullable<StreamSubscribeParams['format']>;

/** A command rejected by the engine with a structured diagnostic (an `error` Response).
 *  `Error.message` is the composite `code: message (help)` for console and logs; UI code reads the
 *  parts (`code`, `diagnosticMessage`, `help`, `related`) so none is shown twice. */
export class ProtocolError extends Error {
  readonly code: string;
  /** The diagnostic's own message, without the code or the help. */
  readonly diagnosticMessage: string;
  readonly path: string | null;
  readonly help: string | null;
  readonly related: readonly WireErrorSchema[];

  constructor(diagnostic: WireErrorSchema) {
    super(diagnostic.help ? `${diagnostic.code}: ${diagnostic.message} (${diagnostic.help})` : `${diagnostic.code}: ${diagnostic.message}`);
    this.name = 'ProtocolError';
    this.code = diagnostic.code;
    this.diagnosticMessage = diagnostic.message;
    this.path = diagnostic.path ?? null;
    this.help = diagnostic.help ?? null;
    this.related = diagnostic.related ?? [];
  }
}

/** Transport/handshake-level failure — malformed messages, version mismatch, closed socket. */
export class ProtocolClientError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ProtocolClientError';
  }
}

export type IncomingMessage =
  | { readonly kind: 'response'; readonly response: Response }
  | { readonly kind: 'event'; readonly event: EventMessage };

/** A success response with its applied frame — the envelope's `ok` body plus the `frame` stamp. */
export interface FullResponse {
  readonly body: ResponseBody;
  /** The sim frame the command applied at, or `null` for responses that carry none. */
  readonly frame: number | null;
  /**
   * The gameplay frame at the same boundary (`frame` less every frame the game has held), or `null`
   * for responses that carry none. This is what a presentation `started` stamp uses.
   */
  readonly gameFrame: number | null;
}

/**
 * Classifies a parsed text message as a `Response` or an `EventMessage` by shape: events carry
 * `event` + `sub` + `seq`; responses carry `id`. Pure and socket-free so demux logic is unit
 * testable against mocked message shapes.
 */
export function classifyMessage(raw: string): IncomingMessage {
  let parsed: unknown;
  try {
    parsed = parseWire(raw);
  } catch (cause) {
    throw new ProtocolClientError(`malformed JSON from server: ${String(cause)}`);
  }
  if (typeof parsed !== 'object' || parsed === null) {
    throw new ProtocolClientError(`expected a JSON object from server, got: ${raw}`);
  }
  const obj = parsed as Record<string, unknown>;
  if ('event' in obj && 'sub' in obj && 'seq' in obj) {
    return { kind: 'event', event: obj as unknown as EventMessage };
  }
  if ('id' in obj && typeof obj.id === 'number') {
    return { kind: 'response', response: obj as unknown as Response };
  }
  throw new ProtocolClientError(`unrecognized message shape from server: ${raw}`);
}

interface PendingCall {
  readonly resolve: (result: FullResponse) => void;
  readonly reject: (error: Error) => void;
}

type Unsubscribe = () => void;

/**
 * A connected session with one engine. Construct, then `await connect(url)` before calling
 * `call`/`subscribe`. Works unmodified in a plain browser tab or inside the Tauri webview — it
 * only uses the standard `WebSocket` API.
 */
export class ProtocolClient {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private readonly pending = new Map<number, PendingCall>();
  private readonly eventListeners = new Set<(event: EventMessage) => void>();
  private readonly frameListeners = new Set<(frame: DecodedFrame) => void>();
  private readonly closeListeners = new Set<() => void>();
  private _serverHello: ServerHello | null = null;
  private _url: string | null = null;

  /** The handshake reply, once connected. */
  get serverHello(): ServerHello | null {
    return this._serverHello;
  }

  /** The URL {@link connect} was called with, or `null` before it was. Kept because WHERE the server
   *  is is a real input to how this client talks to it — `viewer/frameFormat.ts` picks the frame
   *  codec from it, since a loopback socket makes the uncompressed lane the cheap one. */
  get url(): string | null {
    return this._url;
  }

  get isConnected(): boolean {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN;
  }

  /** Opens the socket, sends `hello`, and resolves with the validated `ServerHello`. Always settles:
   *  a close before the reply, or no reply within `timeoutMs`, rejects. */
  connect(url: string, timeoutMs: number = CONNECT_TIMEOUT_MS): Promise<ServerHello> {
    if (this.ws !== null) {
      return Promise.reject(new ProtocolClientError('connect() called on an already-connecting/connected client'));
    }
    this._url = url;
    return new Promise((resolve, reject) => {
      let handshakeDone = false;
      const ws = new WebSocket(url);
      ws.binaryType = 'arraybuffer';
      this.ws = ws;

      const timer = setTimeout(() => {
        settle(() => reject(new ProtocolClientError(`no handshake reply from ${url} within ${timeoutMs} ms`)));
        ws.close();
      }, timeoutMs);
      /** Every exit of the handshake goes through here, so nothing settles twice or not at all. */
      const settle = (outcome: () => void): void => {
        if (handshakeDone) return;
        handshakeDone = true;
        clearTimeout(timer);
        outcome();
      };
      const fail = (err: Error): void => {
        settle(() => reject(err));
        ws.close();
      };

      ws.addEventListener('open', () => {
        const hello: Hello = { protocol_version: PROTOCOL_VERSION, client: CLIENT_NAME, features: [] };
        ws.send(JSON.stringify(hello));
      });

      ws.addEventListener('message', (ev: MessageEvent) => {
        if (!handshakeDone) {
          if (typeof ev.data !== 'string') {
            fail(new ProtocolClientError('expected a text ServerHello as the first message, got binary'));
            return;
          }
          let hello: ServerHello;
          try {
            hello = JSON.parse(ev.data) as ServerHello;
          } catch (cause) {
            fail(new ProtocolClientError(`malformed ServerHello JSON: ${String(cause)}`));
            return;
          }
          const refusal = (hello as unknown as { error?: WireErrorSchema }).error;
          if (refusal !== undefined) {
            fail(new ProtocolError(refusal));
            return;
          }
          if (hello.protocol_version !== PROTOCOL_VERSION) {
            fail(
              new ProtocolClientError(
                `server speaks protocol_version ${hello.protocol_version}, this client speaks ${PROTOCOL_VERSION}`,
              ),
            );
            return;
          }
          this._serverHello = hello;
          settle(() => resolve(hello));
          return;
        }
        this.handleMessage(ev);
      });

      ws.addEventListener('error', () => {
        settle(() => reject(new ProtocolClientError(`WebSocket error connecting to ${url}`)));
      });

      ws.addEventListener('close', () => {
        this.ws = null;
        settle(() => reject(new ProtocolClientError(`${url} closed the connection before the handshake completed`)));
        const closeError = new ProtocolClientError('connection closed');
        for (const p of this.pending.values()) {
          p.reject(closeError);
        }
        this.pending.clear();
        this.notify(this.closeListeners, undefined);
      });
    });
  }

  /** Sends one command and resolves with its success payload, or rejects with a typed error. */
  async call(command: Command): Promise<ResponseBody> {
    const { body } = await this.callFull(command);
    return body;
  }

  /**
   * Like {@link call}, but resolves with the whole {@link FullResponse} — the `ok` body plus the
   * `frame` the command applied at and the `gameFrame` a `started` stamp uses. Used where the applied
   * frame matters (e.g. reporting the frame a `sim.hash` was taken at); most callers want the terser
   * `call`.
   */
  callFull(command: Command): Promise<FullResponse> {
    const ws = this.ws;
    if (ws === null || ws.readyState !== WebSocket.OPEN) {
      return Promise.reject(new ProtocolClientError('call() on a client that is not connected'));
    }
    const id = this.nextId++;
    const request = { v: PROTOCOL_VERSION, id, ...command } as Request;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      ws.send(stringifyWire(request));
    });
  }

  /**
   * Convenience wrapper over `stream.subscribe`; resolves with the new subscription id.
   *
   * `format` picks the payload codec for a `frames` subscription and is ignored on every other
   * channel; omitting it takes the wire default (`QOI`), which is what every non-frame caller wants.
   * A frame subscriber that cares should resolve one via `viewer/frameFormat.ts` rather than pick.
   */
  async subscribe(channel: Channel, mode?: SubscribeMode, format?: SubscribeFormat): Promise<number> {
    const params: StreamSubscribeParams = {
      channel,
      ...(mode === undefined ? {} : { mode }),
      ...(format === undefined ? {} : { format }),
    };
    const body = await this.call({ method: 'stream.subscribe', params });
    if (body.kind !== 'subscribed') {
      throw new ProtocolClientError(`expected a 'subscribed' response, got '${body.kind}'`);
    }
    return body.sub;
  }

  /** Convenience wrapper over `stream.unsubscribe`. */
  async unsubscribe(sub: number): Promise<void> {
    await this.call({ method: 'stream.unsubscribe', params: { sub } });
  }

  onEvent(handler: (event: EventMessage) => void): Unsubscribe {
    this.eventListeners.add(handler);
    return () => this.eventListeners.delete(handler);
  }

  onFrame(handler: (frame: DecodedFrame) => void): Unsubscribe {
    this.frameListeners.add(handler);
    return () => this.frameListeners.delete(handler);
  }

  onClose(handler: () => void): Unsubscribe {
    this.closeListeners.add(handler);
    return () => this.closeListeners.delete(handler);
  }

  close(): void {
    this.ws?.close();
  }

  /** Calls every listener in its own try/catch: one that throws must not starve the rest. */
  private notify<T>(listeners: Set<(value: T) => void>, value: T): void {
    for (const fn of listeners) {
      try {
        fn(value);
      } catch (err) {
        console.error(err);
      }
    }
  }

  private handleMessage(ev: MessageEvent): void {
    if (typeof ev.data === 'string') {
      let msg: IncomingMessage;
      try {
        msg = classifyMessage(ev.data);
      } catch (err) {
        // A malformed message from the server shouldn't take down the whole client. It has no
        // `sub`/`seq` to route on, so it is logged and dropped.
        console.error(err);
        return;
      }
      if (msg.kind === 'response') {
        this.resolveResponse(msg.response);
      } else {
        this.notify(this.eventListeners, msg.event);
      }
      return;
    }

    if (ev.data instanceof ArrayBuffer) {
      let frame: DecodedFrame;
      try {
        frame = decodeFrameHeader(ev.data);
      } catch (err) {
        console.error(err);
        return;
      }
      this.notify(this.frameListeners, frame);
    }
  }

  private resolveResponse(response: Response): void {
    const pending = this.pending.get(response.id);
    if (pending === undefined) {
      // Unknown id: either a duplicate/late reply or a server bug. Not fatal to the session.
      console.warn(`response for unknown request id ${response.id}`);
      return;
    }
    this.pending.delete(response.id);
    // The generated `Response1` union carries a `[k: string]: unknown` index signature on both
    // arms (a json-schema-to-typescript artifact), which defeats TS's usual `'error' in x`
    // discriminant narrowing -- both arms end up structurally compatible with either property
    // access. Read the two possible keys directly instead of relying on narrowing.
    const body = response as unknown as { ok?: ResponseBody; error?: WireErrorSchema; frame?: number | null; game_frame?: number | null };
    if (body.error !== undefined) {
      pending.reject(new ProtocolError(body.error));
    } else if (body.ok !== undefined) {
      pending.resolve({ body: body.ok, frame: body.frame ?? null, gameFrame: body.game_frame ?? null });
    } else {
      pending.reject(new ProtocolClientError(`response ${response.id} has neither 'ok' nor 'error'`));
    }
  }
}
