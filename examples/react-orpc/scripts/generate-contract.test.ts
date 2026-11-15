import { describe, expect, it } from "vitest";

import { generate } from "./generate-contract";

type Json = Record<string, any>;

/** A minimal OpenAPI document around the given operations and schemas. */
function spec(paths: Json, schemas: Json = {}): Json {
  return { openapi: "3.1.0", info: { title: "T", version: "1" }, paths, components: { schemas } };
}

const ok = (schema: Json) => ({ responses: { 200: { content: { "application/json": { schema } } } } });
const op = (id: string, rest: Json) => ({ operationId: id, ...rest });

describe("contract generator", () => {
  it("groups operations by controller and strips the shared path prefix", async () => {
    const out = await generate(
      spec({
        "/api/pets": { get: op("PetsController_list", ok({ type: "array", items: { type: "string" } })) },
        "/api/pets/{id}": {
          get: op("PetsController_find", {
            parameters: [{ name: "id", in: "path", required: true, schema: { type: "string", format: "uuid" } }],
            ...ok({ type: "string" }),
          }),
        },
      }),
    );
    expect(out).toContain('apiPrefix = "/api"');
    expect(out).toMatch(/pets: \{\s+list: oc/);
    expect(out).toContain('path: "/pets/{id}"');
    expect(out).toContain("id: z.uuid()");
    expect(out).toContain(".output(z.array(z.string()))");
  });

  it("merges path parameters into a $ref body and honours the success status", async () => {
    const out = await generate(
      spec(
        {
          "/p/{id}": {
            put: op("PetsController_rename", {
              parameters: [{ name: "id", in: "path", required: true, schema: { type: "string" } }],
              requestBody: { content: { "application/json": { schema: { $ref: "#/components/schemas/Rename" } } } },
              responses: { 204: { description: "done" } },
            }),
          },
        },
        { Rename: { type: "object", required: ["name"], properties: { name: { type: "string", minLength: 1 } } } },
      ),
    );
    expect(out).toContain("...Rename.shape");
    expect(out).toContain("successStatus: 204");
    expect(out).toContain(".output(z.void())");
    expect(out).toContain("name: z.string().min(1)");
  });

  it("turns a tagged oneOf into a discriminated union and an event stream into eventIterator", async () => {
    const frame = (event: string, data: Json) => ({
      type: "object",
      required: ["event", "data"],
      properties: { event: { type: "string", enum: [event] }, data },
    });
    const out = await generate(
      spec(
        {
          "/e": {
            get: op("FeedController_events", {
              responses: {
                200: {
                  content: {
                    "text/event-stream": {
                      schema: {
                        oneOf: [frame("message", { $ref: "#/components/schemas/Ev" }), frame("close", {}), frame("error", {})],
                      },
                    },
                  },
                },
              },
            }),
          },
        },
        {
          Ev: {
            oneOf: [
              { type: "object", required: ["type"], properties: { type: { type: "string", enum: ["a"] } } },
              { type: "object", required: ["type"], properties: { type: { type: "string", enum: ["b"] } } },
            ],
          },
        },
      ),
    );
    expect(out).toContain('z.discriminatedUnion("type"');
    expect(out).toContain(".output(eventIterator(Ev))");
  });

  it("orders schemas so every one is defined before it is used", async () => {
    const out = await generate(
      spec(
        { "/x": { get: op("XController_get", ok({ $ref: "#/components/schemas/A" })) } },
        {
          A: { type: "object", properties: { b: { $ref: "#/components/schemas/B" } } },
          B: { type: "object", properties: { n: { type: ["integer", "null"] } } },
        },
      ),
    );
    expect(out.indexOf("export const B")).toBeLessThan(out.indexOf("export const A"));
    expect(out).toContain("n: z.number().int().nullable().optional()");
  });

  it("leaves nullable query parameters optional rather than nullable", async () => {
    const out = await generate(
      spec({
        "/x": {
          get: op("XController_list", {
            parameters: [{ name: "limit", in: "query", schema: { type: ["integer", "null"], minimum: 0 } }],
            ...ok({ type: "string" }),
          }),
        },
      }),
    );
    expect(out).toContain("limit: z.number().int().min(0).optional()");
  });

  describe("typed errors", () => {
    const envelope = {
      Envelope: { type: "object", required: ["error"], properties: { error: { $ref: "#/components/schemas/Body" } } },
      Body: { type: "object", required: ["code"], properties: { code: { type: "string" } } },
    };
    const failing = (responses: Json) => ({ "/x": { get: op("XController_get", { responses }) } });
    const error = (description: string) => ({
      description,
      content: { "application/json": { schema: { $ref: "#/components/schemas/Envelope" } } },
    });

    it("maps 4xx/5xx responses to oRPC codes whose data is the envelope's body", async () => {
      const out = await generate(
        spec(failing({ 200: { content: { "application/json": { schema: { type: "string" } } } }, 404: error("No such x"), 409: error("Taken"), 500: error("Boom") }), envelope),
      );
      expect(out).toContain('NOT_FOUND: { message: "No such x", data: Body }');
      expect(out).toContain('CONFLICT: { message: "Taken", data: Body }');
      expect(out).toContain("INTERNAL_SERVER_ERROR");
      expect(out.indexOf("NOT_FOUND")).toBeLessThan(out.indexOf("CONFLICT")); // by status, deterministic
    });

    it("emits no .errors() for an operation that declares none", async () => {
      const out = await generate(spec(failing({ 200: { content: { "application/json": { schema: { type: "string" } } } } })));
      expect(out).not.toContain(".errors(");
    });

    it.each([
      ["a status oRPC has no code for", { 200: { description: "ok" }, 418: error("teapot") }, /418/],
      ["a `default` response", { 200: { description: "ok" }, default: error("any") }, /default/],
      [
        "an error that is not the envelope",
        { 200: { description: "ok" }, 404: { description: "x", content: { "application/json": { schema: { type: "string" } } } } },
        /envelope/,
      ],
    ])("refuses %s", async (_name, responses, message) => {
      await expect(generate(spec(failing(responses as Json), envelope))).rejects.toThrow(message as RegExp);
    });
  });

  it("is deterministic", async () => {
    const document = spec({ "/x": { get: op("XController_get", ok({ type: "string" })) } });
    expect(await generate(document)).toBe(await generate(document));
  });

  it.each([
    ["a recursive schema", { "/x": { get: op("X_get", ok({ $ref: "#/components/schemas/T" })) } }, { T: { type: "object", properties: { next: { $ref: "#/components/schemas/T" } } } }, /recursive/],
    ["allOf", { "/x": { get: op("X_get", ok({ allOf: [{ type: "string" }] })) } }, {}, /allOf/],
    [
      "a header parameter",
      { "/x": { get: op("X_get", { parameters: [{ name: "h", in: "header", schema: { type: "string" } }], ...ok({ type: "string" }) }) } },
      {},
      /header/,
    ],
    ["a non-JSON response", { "/x": { get: op("X_get", { responses: { 200: { content: { "text/plain": { schema: { type: "string" } } } } } }) } }, {}, /text\/plain/],
    ["no success response", { "/x": { get: op("X_get", { responses: { 500: {} } }) } }, {}, /2xx/],
    ["an unnamed operation", { "/x": { get: { ...ok({ type: "string" }) } } }, {}, /operationId/],
  ])("refuses %s instead of guessing", async (_name, paths, schemas, message) => {
    await expect(generate(spec(paths as Json, schemas as Json))).rejects.toThrow(message as RegExp);
  });
});
