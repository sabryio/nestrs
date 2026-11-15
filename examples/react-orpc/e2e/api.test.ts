/**
 * The typed client against the real Rust binary: CRUD through `OpenAPILink`, the
 * Rust error envelope decoded into `ORPCError`, and typed Server-Sent Events,
 * including the terminal `close` frame when the server shuts down.
 */
import { ORPCError, isDefinedError, safe } from "@orpc/client";
import { afterAll, beforeAll, describe, expect, expectTypeOf, it } from "vitest";

import { createClient } from "../src/client";
import type { ErrorBody, PostEventDto } from "../src/contract.generated";
import { startProxy } from "./proxy";
import { type RunningServer, startServer } from "./server";

let server: RunningServer;
let api: ReturnType<typeof createClient>;

beforeAll(async () => {
  server = await startServer();
  api = createClient({ origin: server.origin, token: () => "dev-token" });
});

afterAll(async () => {
  await server.stop();
});

/** The error a call fails with, as thrown (untyped); for undeclared failures. */
async function caught(promise: Promise<unknown>): Promise<ORPCError<string, ErrorBody>> {
  try {
    await promise;
  } catch (error) {
    expect(error).toBeInstanceOf(ORPCError);
    return error as ORPCError<string, ErrorBody>;
  }
  throw new Error("expected the call to fail");
}

describe("REST through the contract", () => {
  it("creates, reads, lists and deletes", async () => {
    expect(await api.health.live()).toEqual({ status: "ok" });
    const ready = await api.health.ready();
    expect(ready.status).toBe("ok");
    // `redis` only appears when BLOG_REDIS_URL is set.
    expect(ready.checks.map((check) => check.name).sort()).toEqual(["database"]);

    const ada = await api.users.create({ name: "Ada", email: "ada@example.com" });
    expect(await api.users.find({ id: ada.id })).toEqual(ada);

    const post = await api.posts.create({ author_id: ada.id, title: "Hello", body: "World" });
    expect(post).toMatchObject({ author_id: ada.id, title: "Hello" });
    expect(await api.posts.find({ id: post.id })).toEqual(post);
    expect((await api.posts.list({ limit: 10 })).map((p) => p.id)).toContain(post.id);

    expect(await api.posts.remove({ id: post.id })).toBeUndefined(); // 204
    const [gone] = await safe(api.posts.find({ id: post.id }));
    expect(gone).toBeInstanceOf(ORPCError);
    expect((gone as ORPCError<string, unknown>).code).toBe("NOT_FOUND");
  });

  it("decodes the server's error envelope", async () => {
    const missing = await caught(api.posts.find({ id: crypto.randomUUID() }));
    expect(missing.code).toBe("NOT_FOUND");
    expect(missing.data.code).toBe("not_found");

    const anonymous = createClient({ origin: server.origin, token: () => undefined });
    const denied = await caught(anonymous.users.create({ name: "Eve", email: "eve@example.com" }));
    expect(denied.code).toBe("UNAUTHORIZED");

    const invalid = await caught(api.users.create({ name: "", email: "not-an-email" }));
    expect(invalid.code).toBe("UNPROCESSABLE_CONTENT");
    expect(invalid.data.code).toBe("validation_failed");

    const page = await caught(api.posts.list({ limit: 1000 }));
    expect(page.code).toBe("BAD_REQUEST");
  });
});

describe("typed errors from the contract", () => {
  it("narrows declared failures to their codes, with typed data", async () => {
    // `posts.find` declares exactly one failure: 404.
    const [error, data, definedError] = await safe(api.posts.find({ id: crypto.randomUUID() }));
    expect(data).toBeUndefined();
    expect(definedError).toBe(error); // `safe` hands declared failures back as the third item
    if (!isDefinedError(error)) throw new Error("NOT_FOUND is declared, so it must be a defined error");

    expectTypeOf(error.code).toEqualTypeOf<"NOT_FOUND">();
    expectTypeOf(error.data).toEqualTypeOf<ErrorBody>();
    expect(error.code).toBe("NOT_FOUND");
    expect(error.message).toMatch(/not found/); // the server's own message, not the spec's description
    expect(error.data.code).toBe("not_found"); // the server's stable machine code
  });

  it("answers FORBIDDEN, typed, when the caller's role is not enough", async () => {
    const author = createClient({ origin: server.origin, token: () => "author-token" });
    const [error] = await safe(author.users.create({ name: "Eve", email: "eve@example.com" }));
    if (!isDefinedError(error)) throw new Error("403 is declared on users.create");
    expect(error.code).toBe("FORBIDDEN");
    expect(error.data.code).toBe("forbidden");
  });

  it("types every declared code of a call, and only those", async () => {
    const [error] = await safe(api.users.create({ name: "", email: "x@example.com" }));
    if (!isDefinedError(error)) throw new Error("validation failures are declared");
    // Declared on `users.create`: BAD_REQUEST, UNAUTHORIZED, FORBIDDEN, CONFLICT, TOO_MANY_REQUESTS, UNPROCESSABLE_CONTENT.
    expectTypeOf(error.code).toEqualTypeOf<
      "BAD_REQUEST" | "UNAUTHORIZED" | "FORBIDDEN" | "CONFLICT" | "TOO_MANY_REQUESTS" | "UNPROCESSABLE_CONTENT"
    >();
    expect(error.code).toBe("UNPROCESSABLE_CONTENT");
    expect(error.data.code).toBe("validation_failed");
    expect(error.data.details).toBeDefined();

    // A duplicate email is the declared CONFLICT.
    await api.users.create({ name: "Dup", email: "dup@example.com" });
    const [conflict] = await safe(api.users.create({ name: "Dup", email: "dup@example.com" }));
    if (!isDefinedError(conflict)) throw new Error("CONFLICT is declared");
    expect(conflict.code).toBe("CONFLICT");
  });

  it("leaves failures the contract does not declare as plain errors", async () => {
    // `posts.list` declares no failures, yet the server rejects an oversized page.
    const [error, , definedError] = await safe(api.posts.list({ limit: 1000 }));
    expect(definedError).toBeNull();
    expect(isDefinedError(error)).toBe(false);
    expect(error).toBeInstanceOf(ORPCError);
    expect((error as ORPCError<string, ErrorBody>).code).toBe("BAD_REQUEST");
    expect((error as ORPCError<string, ErrorBody>).data.code).toBe("bad_request");
  });
});

describe("typed Server-Sent Events", () => {
  it("delivers typed events in order", async () => {
    const ada = await api.users.create({ name: "Sse", email: "sse@example.com" });
    const stream = await api.posts.events();
    const seen: PostEventDto[] = [];
    const reader = (async () => {
      for await (const event of stream) {
        seen.push(event);
        if (event.type === "removed") break; // leaving the loop cancels the request
      }
    })();

    const post = await api.posts.create({ author_id: ada.id, title: "Live", body: "Now" });
    await api.posts.remove({ id: post.id });
    await reader;

    expect(seen.map((event) => event.type)).toEqual(["published", "removed"]);
    const [first, second] = seen;
    // The union narrows: `post` only exists on `published`, `id` on `removed`.
    if (first?.type === "published") expect(first.post.title).toBe("Live");
    else throw new Error("first event should be published");
    if (second?.type === "removed") expect(second.id).toBe(post.id);
    else throw new Error("second event should be removed");
  });

  it("sends flush, then messages, then close, byte for byte", async () => {
    const response = await fetch(`${server.origin}/api/posts/events`, {
      headers: { accept: "text/event-stream" },
    });
    expect(response.headers.get("content-type")).toBe("text/event-stream");
    const reader = response.body!.getReader();
    const decoder = new TextDecoder();
    let text = "";
    const until = async (needle: string) => {
      while (!text.includes(needle)) {
        const { done, value } = await reader.read();
        if (done) throw new Error(`ended before ${JSON.stringify(needle)}: ${JSON.stringify(text)}`);
        text += decoder.decode(value, { stream: true });
      }
    };

    await until(": \n\n"); // headers + flush arrive before any event exists
    expect(text).toBe(": \n\n");

    const ada = await api.users.create({ name: "Raw", email: "raw@example.com" });
    const post = await api.posts.create({ author_id: ada.id, title: "Raw", body: "Bytes" });
    await until("\n\n"); // (the flush already ended in a blank line; the id check below waits for the message)
    await until(`"id":"${post.id}"`);
    // flush, then a message whose `id:` line precedes its `data:` line
    expect(text).toMatch(/^: \n\nid: \d+-\d+\ndata: \{"type":"published","post":\{/);
    await reader.cancel();
  });
});

describe("reconnecting event streams", () => {
  it("resumes after a dropped connection with no gap and no duplicate", async () => {
    const proxy = await startProxy(server.origin);
    // The stream goes through the proxy; the writes go straight to the server.
    const viaProxy = createClient({ origin: proxy.origin, reconnectDelay: 50 });
    const author = await api.users.create({ name: "Flaky", email: "flaky@example.com" });
    const publish = (title: string) => api.posts.create({ author_id: author.id, title, body: "b" });

    const titles: string[] = [];
    const waitFor = async (title: string) => {
      const deadline = Date.now() + 10_000;
      while (!titles.includes(title)) {
        if (Date.now() > deadline) throw new Error(`never saw ${title}; saw ${titles}`);
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    };
    const stream = await viaProxy.posts.events();
    const reader = (async () => {
      for await (const event of stream) {
        if (event.type === "published" && event.post.title.startsWith("r-")) titles.push(event.post.title);
        if (titles.length === 4) break;
      }
    })();

    await publish("r-1");
    await waitFor("r-1");

    proxy.block(); // the network goes away...
    proxy.sever();
    await publish("r-2"); // ...and the server keeps publishing
    await publish("r-3");
    proxy.allow(); // ...then comes back
    await publish("r-4");

    await reader;
    expect(titles).toEqual(["r-1", "r-2", "r-3", "r-4"]);
    await proxy.close();
  });
});

describe("graceful shutdown", () => {
  it("ends open streams with `close` instead of hanging", async () => {
    const own = await startServer();
    const client = createClient({ origin: own.origin });
    const stream = await client.posts.events();
    let completed = false;
    const consume = (async () => {
      for await (const _event of stream) {
        /* no events expected */
      }
      completed = true; // the iterator finished without throwing: it saw `close`
    })();

    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(completed).toBe(false);

    own.process.kill("SIGTERM");
    await consume;
    expect(completed).toBe(true);
    expect(await own.exited).toBe(0);
  });
});
