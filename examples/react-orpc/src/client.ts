import { EventStreamErrorEvent, createORPCClient } from "@orpc/client";
import { RetryLinkPlugin, isRetryableError } from "@orpc/client/plugins";
import type { ContractRouterClient } from "@orpc/contract";
import { OpenAPILink } from "@orpc/openapi/fetch";
import { createTanstackQueryUtils } from "@orpc/tanstack-query";
import { QueryClient } from "@tanstack/react-query";

import { apiPrefix, contract } from "./contract.generated";
import { decodePurlinError, markDefinedErrors } from "./errors";

export interface ClientOptions {
  /** Origin of the API; leave unset in the browser (the Vite proxy handles it). */
  origin?: string;
  /** Path the Rust app is nested under (`App::global_prefix`). */
  prefix?: `/${string}`;
  /** Bearer token for writes; read per call, so it can change without a new client. */
  token?: () => string | undefined;
  /** Milliseconds to wait before reconnecting a dropped event stream. Default 1000. */
  reconnectDelay?: number;
  /** Called after a dropped event stream has reconnected. */
  onReconnect?: () => void;
}

/**
 * A typed client for the purlin blog-api, built from the contract alone.
 *
 * Event streams reconnect by themselves. The retry plugin remembers the `id` of the
 * last message it delivered and sends it back as `Last-Event-ID`, so the server
 * replays what was missed and the iterator the caller is looping over just keeps
 * going: no gap, no duplicate, no code in the UI.
 */
export function createClient(options: ClientOptions = {}): ContractRouterClient<typeof contract> {
  const link = new OpenAPILink(contract, {
    url: options.prefix ?? apiPrefix,
    origin: options.origin,
    headers: () => {
      const token = options.token?.();
      return token ? { authorization: `Bearer ${token}` } : {};
    },
    errorResponseBodyDecoder: decodePurlinError,
    interceptors: [markDefinedErrors(contract)],
    plugins: [
      new RetryLinkPlugin({
        default: {
          // Only streams reconnect; a failed REST call should fail, not repeat.
          retry: ({ path }) => (path.join(".") === "posts.events" ? Number.POSITIVE_INFINITY : 0),
          retryDelay: options.reconnectDelay ?? 1000,
          // An `event: error` frame is the server saying the stream failed for good;
          // a dropped connection or a missing `close` is worth another try.
          shouldRetry: ({ error }) => !(error instanceof EventStreamErrorEvent) && isRetryableError(error),
          onRetry: () => (isSuccess) => {
            if (isSuccess) options.onReconnect?.();
          },
        },
      }),
    ],
  });
  return createORPCClient(link);
}

export const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});

/** The default client for the app, plus its TanStack Query helpers. */
export const client = createClient({
  token: () => sessionStorage.getItem("token") ?? "dev-token",
  // Replay is best effort (the server keeps a bounded history), so after a
  // reconnect the list is refetched: whatever the replay could not cover is repaired.
  onReconnect: () => void queryClient.invalidateQueries({ queryKey: orpc.posts.list.key() }),
});
export const orpc = createTanstackQueryUtils(client);
