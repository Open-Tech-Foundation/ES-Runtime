declare module "runtime:http" {
  /**
   * An HTTP request handler: called with a web `Request`, returns (or resolves
   * to) a web `Response`. A thrown error or a non-`Response` return becomes a
   * `500`.
   */
  export type Handler = (request: Request) => Response | Promise<Response>;

  /** Options for {@link serve}. */
  export interface ServeOptions {
    /** Address to bind. Defaults to `"0.0.0.0"`. */
    hostname?: string;
    host?: string;
    /** `0` (the default) binds an ephemeral port (read it back from `addr`). */
    port?: number;
  }

  /** A running HTTP server. */
  export interface Server {
    /** The bound address (resolves once the server is listening). */
    readonly addr: Promise<{ hostname: string; port: number }>;
    /** Resolves when the accept loop has ended (after {@link stop}). */
    readonly finished: Promise<void>;
    /** Stop accepting and shut the server down; resolves once stopped. */
    stop(): Promise<void>;
  }

  /** Start an HTTP/1.1 server (capability: `NetListen`). Returns immediately. */
  export function serve(handler: Handler): Server;
  export function serve(options: ServeOptions, handler: Handler): Server;

  const http: { serve: typeof serve };
  export default http;
}
