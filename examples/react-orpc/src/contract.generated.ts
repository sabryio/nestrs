/* eslint-disable */
// GENERATED FILE: do not edit.
// Source: openapi.json (`Blog API 0.1.0`), produced by `blog-api --print-openapi`.
// Regenerate with `npm run gen`; `npm run check:contract` fails when this file is stale.

import "@orpc/openapi/extensions/route";
import { eventIterator, oc } from "@orpc/contract";
import { z } from "zod";

/** Where the server mounts the API; pass it as the link's `url`. */
export const apiPrefix = "/api" as `/${string}`;

// ---------------------------------------------------------------- schemas

/** Outcome of one check. */
export const Status = z.enum(["up", "down"]);
export type Status = z.infer<typeof Status>;

/** One indicator's result. */
export const CheckResult = z.object({
  critical: z.boolean().describe("Whether its failure makes the service not ready."),
  error: z
    .string()
    .nullable()
    .describe("Why it failed; absent when healthy or when details are hidden.")
    .optional(),
  latency_ms: z.number().int().min(0).describe("How long the check took, in milliseconds."),
  name: z.string().describe("The indicator's name."),
  status: Status.describe("Whether it is healthy."),
});
export type CheckResult = z.infer<typeof CheckResult>;

/** Request body for `POST /posts`. */
export const CreatePostRequest = z.object({
  author_id: z.uuid().describe("Must be an existing user."),
  body: z.string().min(1).describe("Body text, at least one character."),
  title: z.string().min(1).max(200).describe("Headline, 1–200 characters."),
});
export type CreatePostRequest = z.infer<typeof CreatePostRequest>;

/** Request body for `POST /users`. */
export const CreateUserRequest = z.object({
  email: z.email().describe("Email address; compared case-insensitively."),
  name: z.string().min(1).max(100).describe("Display name, 1–100 characters."),
});
export type CreateUserRequest = z.infer<typeof CreateUserRequest>;

/** Body of [`ErrorEnvelope`]. */
export const ErrorBody = z.object({
  code: z.string().describe("Stable machine-readable code (`not_found`, `validation_failed`, ..)."),
  details: z.unknown().describe("Per-field details for validation failures.").optional(),
  message: z.string().describe("Human-readable message."),
});
export type ErrorBody = z.infer<typeof ErrorBody>;

/** The error envelope every failure uses (see `HttpError`). */
export const ErrorEnvelope = z.object({
  error: ErrorBody,
});
export type ErrorEnvelope = z.infer<typeof ErrorEnvelope>;

/** Body of the liveness probe. */
export const Liveness = z.object({
  status: z.string().describe('Always `"ok"` while the process is serving.'),
});
export type Liveness = z.infer<typeof Liveness>;

/** A post as returned to clients. */
export const PostResponse = z.object({
  author_id: z.uuid().describe("The author's user id."),
  body: z.string().describe("Body text."),
  id: z.uuid().describe("Unique identifier."),
  title: z.string().describe("Headline."),
});
export type PostResponse = z.infer<typeof PostResponse>;

/** One event on `GET /posts/events`, discriminated by `type`. This is the stream's contract: the handler returns `EventStream<PostEventDto>`, so the OpenAPI document (and any generated client) knows every shape a message can take. */
export const PostEventDto = z.discriminatedUnion("type", [
  z.object({
    post: PostResponse.describe("The new post."),
    type: z.literal("published"),
  }),
  z.object({
    id: z.uuid().describe("Id of the deleted post."),
    type: z.literal("removed"),
  }),
]);
export type PostEventDto = z.infer<typeof PostEventDto>;

/** Body of a successful readiness probe: every critical check passed. */
export const Readiness = z.object({
  checks: z.array(CheckResult).describe("Every check's result."),
  status: z.string().describe('`"ok"`, or `"degraded"` when only non-critical checks are down.'),
});
export type Readiness = z.infer<typeof Readiness>;

/** A user as returned to clients. */
export const UserResponse = z.object({
  email: z.string().describe("Lower-cased email address."),
  id: z.uuid().describe("Unique identifier."),
  name: z.string().describe("Display name."),
});
export type UserResponse = z.infer<typeof UserResponse>;

// --------------------------------------------------------------- contract

export const contract = {
  health: {
    live: oc
      .route({
        method: "GET",
        path: "/health/live",
        summary:
          "Liveness: the process is up and serving. Touches no dependency, so a failing database never gets the pod restarted.",
        tags: ["HealthController"],
      })
      .output(Liveness),
    ready: oc
      .route({
        method: "GET",
        path: "/health/ready",
        summary:
          "Readiness: every critical dependency is healthy and the app is not shutting down. Answers 503 otherwise, with the per-check results in `error.details`.",
        tags: ["HealthController"],
      })
      .errors({
        SERVICE_UNAVAILABLE: {
          message: "Not ready: a critical check is failing, or the app is draining",
          data: ErrorBody,
        },
      })
      .output(Readiness),
  },
  posts: {
    list: oc
      .route({
        method: "GET",
        path: "/posts",
        summary: "List posts.",
        description: "Paginated with `limit` and `offset`.",
        tags: ["PostsController"],
      })
      .input(
        z.object({
          limit: z.number().int().min(0).describe("Page size, 1 to 100 (default 20).").optional(),
          offset: z.number().int().min(0).describe("Items to skip (default 0).").optional(),
        }),
      )
      .output(z.array(PostResponse)),
    create: oc
      .route({
        method: "POST",
        path: "/posts",
        successStatus: 201,
        summary: "Publish a post as an existing author.",
        description: "Requires role admin or author.",
        tags: ["PostsController"],
      })
      .errors({
        BAD_REQUEST: { message: "Malformed request", data: ErrorBody },
        UNAUTHORIZED: { message: "Missing or invalid bearer token", data: ErrorBody },
        FORBIDDEN: { message: "The caller lacks the required role or permission", data: ErrorBody },
        NOT_FOUND: { message: "The author does not exist", data: ErrorBody },
        UNPROCESSABLE_CONTENT: { message: "The request body failed validation", data: ErrorBody },
        TOO_MANY_REQUESTS: { message: "Too many writes; see the Retry-After header", data: ErrorBody },
      })
      .input(CreatePostRequest)
      .output(PostResponse),
    events: oc
      .route({
        method: "GET",
        path: "/posts/events",
        summary: "Stream post events.",
        description:
          "Server-Sent Events: a flush comment, then one message per event\n(`published` or `removed`) each with an `id`, then `event: close` when the\nserver stops. Reconnect with a `Last-Event-ID` header to resume after that\nevent: what was missed is replayed first, then the stream goes live.",
        tags: ["PostsController"],
      })
      .output(eventIterator(PostEventDto)),
    find: oc
      .route({
        method: "GET",
        path: "/posts/{id}",
        summary: "Fetch one post by id.",
        tags: ["PostsController"],
      })
      .errors({ NOT_FOUND: { message: "No post with this id", data: ErrorBody } })
      .input(
        z.object({
          id: z.uuid(),
        }),
      )
      .output(PostResponse),
    remove: oc
      .route({
        method: "DELETE",
        path: "/posts/{id}",
        successStatus: 204,
        summary: "Delete a post.",
        description: "Requires permission posts:delete.",
        tags: ["PostsController"],
      })
      .errors({
        UNAUTHORIZED: { message: "Missing or invalid bearer token", data: ErrorBody },
        FORBIDDEN: { message: "The caller lacks the required role or permission", data: ErrorBody },
        NOT_FOUND: { message: "No post with this id", data: ErrorBody },
      })
      .input(
        z.object({
          id: z.uuid(),
        }),
      )
      .output(z.void()),
  },
  users: {
    list: oc
      .route({
        method: "GET",
        path: "/users",
        summary: "List users.",
        description: "Paginated with `limit` and `offset`.",
        tags: ["UsersController"],
      })
      .input(
        z.object({
          limit: z.number().int().min(0).describe("Page size, 1 to 100 (default 20).").optional(),
          offset: z.number().int().min(0).describe("Items to skip (default 0).").optional(),
        }),
      )
      .output(z.array(UserResponse)),
    create: oc
      .route({
        method: "POST",
        path: "/users",
        successStatus: 201,
        summary:
          "Register a user. Emails are unique, case-insensitively. The response echoes the request id (the caller's `x-request-id`, or a generated one).",
        description: "Requires role admin.",
        tags: ["UsersController"],
      })
      .errors({
        BAD_REQUEST: { message: "Malformed request", data: ErrorBody },
        UNAUTHORIZED: { message: "Missing or invalid bearer token", data: ErrorBody },
        FORBIDDEN: { message: "The caller lacks the required role or permission", data: ErrorBody },
        CONFLICT: { message: "The email is already registered", data: ErrorBody },
        UNPROCESSABLE_CONTENT: { message: "The request body failed validation", data: ErrorBody },
        TOO_MANY_REQUESTS: { message: "Too many writes; see the Retry-After header", data: ErrorBody },
      })
      .input(CreateUserRequest)
      .output(UserResponse),
    find: oc
      .route({
        method: "GET",
        path: "/users/{id}",
        summary: "Fetch one user by id.",
        tags: ["UsersController"],
      })
      .errors({ NOT_FOUND: { message: "No user with this id", data: ErrorBody } })
      .input(
        z.object({
          id: z.uuid(),
        }),
      )
      .output(UserResponse),
  },
};
