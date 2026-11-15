/**
 * Bridges the Rust server's error envelope and oRPC's typed errors.
 *
 * Every nestrs failure is `{"error":{"code","message","details"?}}`. Two steps turn
 * it into something the contract can type:
 *
 * 1. `decodeNestrsError` (the link's `errorResponseBodyDecoder`) builds an
 *    `ORPCError` whose `code` follows the HTTP status (404 → `NOT_FOUND`) and whose
 *    `data` is the envelope's body, the same shape the generated `ErrorBody` schema
 *    describes. Without it a 404 would surface as an opaque `MalformedResponseError`.
 * 2. `markDefinedErrors` (a link interceptor) flags the error as *defined* when the
 *    procedure's `.errors({...})` declares that code. `isDefinedError(error)` then
 *    narrows to the exact codes that call can fail with, with typed `data`.
 */
import { COMMON_ERROR_STATUS_MAP, ORPCError, createORPCErrorFromJson } from "@orpc/client";
import type { StandardLinkInterceptor } from "@orpc/client/standard";

import type { ErrorBody } from "./contract.generated";

/** oRPC's error code for an HTTP status; unlisted client errors are `BAD_REQUEST`, the rest 500s. */
const codeForStatus = new Map<number, string>(
  Object.entries(COMMON_ERROR_STATUS_MAP).map(([code, status]) => [status as number, code] as const),
);

function codeFor(status: number): string {
  return codeForStatus.get(status) ?? (status >= 500 ? "INTERNAL_SERVER_ERROR" : "BAD_REQUEST");
}

function isNestrsErrorBody(body: unknown): body is { error: ErrorBody } {
  if (typeof body !== "object" || body === null || !("error" in body)) return false;
  const error = (body as { error: unknown }).error;
  return (
    typeof error === "object" &&
    error !== null &&
    typeof (error as { code?: unknown }).code === "string" &&
    typeof (error as { message?: unknown }).message === "string"
  );
}

/** `errorResponseBodyDecoder` for `OpenAPILink`. */
export function decodeNestrsError(
  body: unknown,
  response: { status: number },
  fallback: () => Error,
): Error {
  if (!isNestrsErrorBody(body)) return fallback();
  return new ORPCError(codeFor(response.status), { message: body.error.message, data: body.error });
}

/**
 * Link interceptor: errors whose code the procedure declares become *defined*
 * (`error.defined === true`), which is what `isDefinedError` and `safe` look at.
 * Codes the contract does not declare stay plain `ORPCError`s: they are real
 * failures, just not part of this call's typed surface.
 */
export function markDefinedErrors(contract: object): StandardLinkInterceptor<any> {
  return async ({ next, path }) => {
    try {
      return await next();
    } catch (error) {
      if (!(error instanceof ORPCError) || error.defined) throw error;
      const procedure = path.reduce<any>((node, key) => node?.[key], contract);
      if (!procedure?.["~orpc"]?.errorMap?.[error.code]) throw error;
      throw createORPCErrorFromJson(
        { defined: true, code: error.code, message: error.message, data: error.data },
        { cause: error.cause },
      );
    }
  };
}
