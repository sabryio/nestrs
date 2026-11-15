import { ORPCError } from "@orpc/client";

import type { ErrorBody } from "./contract.generated";

/** One readable line for any failure a call can throw. */
export function describeError(error: unknown): string {
  // A declared failure: `data` is the server's `ErrorBody`.
  if (error instanceof ORPCError && error.defined) {
    const { code, message, details } = error.data as ErrorBody;
    const fields = details && typeof details === "object" ? ` ${JSON.stringify(details)}` : "";
    return `${error.code} (${code}): ${message}${fields}`;
  }
  if (error instanceof ORPCError) return `${error.code}: ${error.message}`;
  return error instanceof Error ? error.message : String(error);
}
