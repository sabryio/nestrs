/**
 * Generates the TypeScript contract from the OpenAPI document the Rust server
 * publishes, so the server is the only place an API is written down.
 *
 *   npm run gen              regenerate openapi.json (from the Rust binary) and the contract
 *   npm run check:contract   fail if either file is out of date (for CI)
 *
 * Input:  openapi.json            (`blog-api --print-openapi`)
 * Output: src/contract.generated.ts
 *
 * What it produces:
 *   - one zod schema (and type) per component schema, dependencies first;
 *   - one procedure per operation, grouped by controller (`PostsController_find`
 *     becomes `contract.posts.find`);
 *   - a compact input: path params + query params + JSON body properties in one object;
 *   - the output of the lowest 2xx response, `z.void()` when it has no body, and
 *     `eventIterator(<message schema>)` for a `text/event-stream` response;
 *   - `.errors({...})` from the 4xx/5xx responses: the status picks oRPC's error code
 *     (404 → NOT_FOUND), the error envelope's body becomes the error's typed `data`.
 *
 * It supports the OpenAPI 3.1 subset the purlin generator emits and stops with a
 * pointed message on anything else, rather than silently emitting `z.any()`.
 */
import { COMMON_ERROR_STATUS_MAP } from "@orpc/client";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { format } from "prettier";

type Json = Record<string, any>;

const root = fileURLToPath(new URL("..", import.meta.url));
const workspace = fileURLToPath(new URL("../../..", import.meta.url));
const specPath = `${root}/openapi.json`;
const contractPath = `${root}/src/contract.generated.ts`;

class Unsupported extends Error {
  constructor(where: string, what: string) {
    super(`${where}: ${what} is not supported by the contract generator`);
  }
}

// ------------------------------------------------------------------ schemas

const refName = (ref: string) => ref.split("/").pop()!;

/** Converts one JSON schema to zod source. `where` names the schema for errors. */
function toZod(schema: Json, where: string): string {
  if (schema.$ref) return refName(schema.$ref);
  if (schema.const !== undefined) return `z.literal(${JSON.stringify(schema.const)})`;

  const alternatives: Json[] | undefined = schema.oneOf ?? schema.anyOf;
  if (alternatives) {
    const members = alternatives.map((alternative, index) => toZod(alternative, `${where}.oneOf[${index}]`));
    const tag = discriminator(alternatives);
    return tag
      ? `z.discriminatedUnion(${JSON.stringify(tag)}, [${members.join(", ")}])`
      : `z.union([${members.join(", ")}])`;
  }
  if (schema.allOf) throw new Unsupported(where, "`allOf`");

  const types: string[] = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : [];
  const nullable = types.includes("null");
  const type = types.find((t) => t !== "null");
  let out: string;

  if (schema.enum) {
    const values: unknown[] = schema.enum.filter((value: unknown) => value !== null);
    out = values.length === 1 ? `z.literal(${JSON.stringify(values[0])})` : `z.enum(${JSON.stringify(values)})`;
  } else if (type === "string") {
    out = stringSchema(schema);
  } else if (type === "integer" || type === "number") {
    out = type === "integer" ? "z.number().int()" : "z.number()";
    if (schema.minimum !== undefined) out += `.min(${schema.minimum})`;
    if (schema.maximum !== undefined) out += `.max(${schema.maximum})`;
  } else if (type === "boolean") {
    out = "z.boolean()";
  } else if (type === "array") {
    out = `z.array(${toZod(schema.items ?? {}, `${where}[]`)})`;
  } else if (type === "object" || schema.properties) {
    out = objectSchema(schema, where);
  } else if (type === undefined) {
    out = "z.unknown()"; // `{}`: any JSON value, e.g. `serde_json::Value`
  } else {
    throw new Unsupported(where, `type \`${type}\``);
  }

  if (nullable) out += ".nullable()";
  return out;
}

function stringSchema(schema: Json): string {
  const formats: Record<string, string> = {
    uuid: "z.uuid()",
    email: "z.email()",
    "date-time": "z.iso.datetime()",
    date: "z.iso.date()",
    uri: "z.url()",
  };
  let out = formats[schema.format as string] ?? "z.string()";
  if (schema.minLength !== undefined) out += `.min(${schema.minLength})`;
  if (schema.maxLength !== undefined) out += `.max(${schema.maxLength})`;
  if (schema.pattern !== undefined) out += `.regex(new RegExp(${JSON.stringify(schema.pattern)}))`;
  return out;
}

function objectSchema(schema: Json, where: string): string {
  const properties: Json = schema.properties ?? {};
  const required = new Set<string>(schema.required ?? []);
  const entries = Object.entries(properties);
  if (entries.length === 0) {
    const extra = schema.additionalProperties;
    const value = extra && typeof extra === "object" ? toZod(extra, `${where}.*`) : "z.unknown()";
    return `z.record(z.string(), ${value})`;
  }
  const fields = entries.map(([name, property]) => {
    let field = toZod(property as Json, `${where}.${name}`);
    if ((property as Json).description) field += `.describe(${JSON.stringify((property as Json).description)})`;
    if (!required.has(name)) field += ".optional()";
    return `  ${JSON.stringify(name)}: ${field},`;
  });
  return `z.object({\n${fields.join("\n")}\n})`;
}

/** The property every alternative pins to one string literal, if there is one. */
function discriminator(alternatives: Json[]): string | undefined {
  const literal = (alternative: Json, key: string): string | undefined => {
    const property = alternative.properties?.[key];
    const values: unknown[] | undefined = property?.enum ?? (property?.const !== undefined ? [property.const] : undefined);
    return values?.length === 1 && typeof values[0] === "string" ? values[0] : undefined;
  };
  if (alternatives.some((alternative) => alternative.$ref || !alternative.properties)) return undefined;
  const keys = Object.keys(alternatives[0]?.properties ?? {});
  return keys.find((key) => {
    const seen = alternatives.map((alternative) => literal(alternative, key));
    return seen.every((value) => value !== undefined) && new Set(seen).size === seen.length;
  });
}

/** Component names a schema refers to, for ordering. */
function refs(schema: unknown, found = new Set<string>()): Set<string> {
  if (Array.isArray(schema)) schema.forEach((item) => refs(item, found));
  else if (schema && typeof schema === "object") {
    for (const [key, value] of Object.entries(schema)) {
      if (key === "$ref" && typeof value === "string") found.add(refName(value));
      else refs(value, found);
    }
  }
  return found;
}

function emitComponents(schemas: Json): { code: string; order: string[] } {
  const order: string[] = [];
  const state = new Map<string, "visiting" | "done">();
  const visit = (name: string, via: string[]) => {
    if (state.get(name) === "done") return;
    if (state.get(name) === "visiting") throw new Unsupported(via.join(" → "), "a recursive schema");
    if (!schemas[name]) throw new Error(`unknown schema \`${name}\` referenced from ${via.join(" → ")}`);
    state.set(name, "visiting");
    for (const dependency of refs(schemas[name])) visit(dependency, [...via, dependency]);
    state.set(name, "done");
    order.push(name);
  };
  Object.keys(schemas).sort().forEach((name) => visit(name, [name]));

  const code = order
    .map((name) => {
      const description = schemas[name].description ? `/** ${oneLine(schemas[name].description)} */\n` : "";
      return `${description}export const ${name} = ${toZod(schemas[name], name)};\nexport type ${name} = z.infer<typeof ${name}>;\n`;
    })
    .join("\n");
  return { code, order };
}

const oneLine = (text: string) => text.replace(/\s*\n\s*/g, " ").replace(/\*\//g, "*\\/").trim();

// --------------------------------------------------------------- operations

interface Operation {
  group: string;
  name: string;
  code: string;
}

function emitOperation(method: string, path: string, prefix: string, operation: Json, spec: Json): Operation {
  const where = `${method.toUpperCase()} ${path}`;
  const id: string = operation.operationId ?? "";
  const [controller, handler] = id.split("_");
  if (!controller || !handler) throw new Error(`${where}: operationId \`${id}\` is not \`<Controller>_<handler>\``);
  const group = lowerFirst(controller.replace(/Controller$/, ""));

  // ---- input: path + query parameters, then the JSON body's properties
  const fields: string[] = [];
  const taken = new Set<string>();
  const add = (name: string, source: string) => {
    if (taken.has(name)) throw new Unsupported(where, `\`${name}\` appearing in both ${source} and another input`);
    taken.add(name);
  };
  for (const parameter of operation.parameters ?? []) {
    if (parameter.in !== "path" && parameter.in !== "query") {
      throw new Unsupported(where, `a \`${parameter.in}\` parameter (\`${parameter.name}\`)`);
    }
    add(parameter.name, `the ${parameter.in} parameters`);
    // `null` cannot travel in a URL; an optional parameter is simply left out.
    const inner = { ...parameter.schema, type: withoutNull(parameter.schema?.type) };
    let field = toZod(inner, `${where} ${parameter.in} \`${parameter.name}\``);
    if (parameter.description) field += `.describe(${JSON.stringify(parameter.description)})`;
    if (!parameter.required) field += ".optional()";
    fields.push(`  ${JSON.stringify(parameter.name)}: ${field},`);
  }

  let input: string | undefined;
  const body = operation.requestBody?.content;
  if (body) {
    const media = body["application/json"];
    if (!media || Object.keys(body).length !== 1) throw new Unsupported(where, "a request body that is not only `application/json`");
    const schema = media.schema?.$ref ? spec.components.schemas[refName(media.schema.$ref)] : media.schema;
    if (!schema?.properties) throw new Unsupported(where, "a JSON body that is not an object");
    for (const name of Object.keys(schema.properties)) add(name, "the body");
    input = fields.length === 0 && media.schema.$ref ? refName(media.schema.$ref) : undefined;
    if (!input) {
      const bodyRef = media.schema.$ref ? `${refName(media.schema.$ref)}.shape` : null;
      if (!bodyRef) throw new Unsupported(where, "an inline request body schema");
      input = `z.object({\n  ...${bodyRef},\n${fields.join("\n")}\n})`;
    }
  } else if (fields.length > 0) {
    input = `z.object({\n${fields.join("\n")}\n})`;
  }

  // ---- output: the lowest 2xx response
  const successes = Object.keys(operation.responses ?? {}).filter((status) => /^2\d\d$/.test(status)).sort();
  const status = successes[0];
  if (!status) throw new Unsupported(where, "an operation without a 2xx response");
  const content = operation.responses[status].content as Json | undefined;
  let output: string;
  if (!content) {
    output = "z.void()";
  } else if (content["text/event-stream"]) {
    output = `eventIterator(${messageSchema(content["text/event-stream"].schema, where)})`;
  } else if (content["application/json"]) {
    output = toZod(content["application/json"].schema ?? {}, `${where} response`);
  } else {
    throw new Unsupported(where, `response content ${Object.keys(content).join(", ")}`);
  }

  const errors = emitErrors(operation.responses, where, spec);

  const route = [
    `method: ${JSON.stringify(method.toUpperCase())}`,
    `path: ${JSON.stringify(path.slice(prefix.length) || "/")}`,
    ...(status !== "200" ? [`successStatus: ${status}`] : []),
    ...(operation.summary ? [`summary: ${JSON.stringify(operation.summary)}`] : []),
    ...(operation.description ? [`description: ${JSON.stringify(operation.description)}`] : []),
    ...(operation.tags?.length ? [`tags: ${JSON.stringify(operation.tags)}`] : []),
  ].join(", ");

  const lines = [`oc.route({ ${route} })`];
  if (errors) lines.push(`.errors(${errors})`);
  if (input) lines.push(`.input(${input})`);
  lines.push(`.output(${output})`);
  return { group, name: handler, code: lines.join("\n      ") };
}

/** oRPC's error code for each HTTP status (the inverse of its own table). */
const codeForStatus = new Map<number, string>(
  Object.entries(COMMON_ERROR_STATUS_MAP).map(([code, status]) => [status as number, code] as const),
);

/**
 * `.errors` map source for an operation's 4xx/5xx responses, or `undefined` when it
 * declares none. Every error response must use the purlin envelope
 * (`{ error: <body> }`); the body schema becomes the error's `data`.
 */
function emitErrors(responses: Json, where: string, spec: Json): string | undefined {
  const entries: string[] = [];
  const statuses = Object.keys(responses).sort();
  for (const status of statuses) {
    if (/^2\d\d$/.test(status)) continue;
    if (!/^[45]\d\d$/.test(status)) throw new Unsupported(where, `the \`${status}\` response`);
    const code = codeForStatus.get(Number(status));
    if (!code) throw new Unsupported(where, `status ${status} (oRPC has no error code for it)`);

    const schema = responses[status].content?.["application/json"]?.schema;
    const envelope = schema?.$ref ? spec.components.schemas[refName(schema.$ref)] : undefined;
    const keys = Object.keys(envelope?.properties ?? {});
    if (keys.length !== 1 || keys[0] !== "error") {
      throw new Unsupported(where, `a ${status} response that is not the \`{ "error": .. }\` envelope`);
    }
    const data = toZod(envelope.properties.error, `${where} ${status} error`);
    const message = responses[status].description;
    entries.push(`${code}: { ${message ? `message: ${JSON.stringify(message)}, ` : ""}data: ${data} }`);
  }
  return entries.length > 0 ? `{ ${entries.join(", ")} }` : undefined;
}

/** The `message` variant's `data` in the stream's `oneOf` (oRPC's event iterator shape). */
function messageSchema(schema: Json, where: string): string {
  const variants: Json[] = schema?.oneOf ?? [];
  const message = variants.find((variant) => variant.properties?.event?.enum?.[0] === "message");
  if (!message?.properties?.data) throw new Unsupported(where, "an event stream without a `message` variant");
  return toZod(message.properties.data, `${where} event`);
}

const lowerFirst = (text: string) => text.charAt(0).toLowerCase() + text.slice(1);
const withoutNull = (type: unknown) => (Array.isArray(type) ? type.filter((t) => t !== "null")[0] : type);

/** The longest path prefix every operation shares (`/api`), so the contract holds only relative routes. */
function commonPrefix(paths: string[]): string {
  const split = paths.map((path) => path.split("/"));
  const first = split[0] ?? [];
  let length = 0;
  while (length < first.length - 1 && split.every((parts) => parts[length] === first[length])) length++;
  return first.slice(0, length).join("/");
}

// -------------------------------------------------------------------- build

export async function generate(spec: Json): Promise<string> {
  if (!String(spec.openapi).startsWith("3.")) throw new Error(`expected OpenAPI 3.x, got ${spec.openapi}`);
  const paths = Object.keys(spec.paths);
  const prefix = commonPrefix(paths);
  const { code: components } = emitComponents(spec.components?.schemas ?? {});

  const groups = new Map<string, Operation[]>();
  for (const path of paths) {
    for (const [method, operation] of Object.entries<Json>(spec.paths[path])) {
      if (!["get", "post", "put", "patch", "delete"].includes(method)) continue;
      const built = emitOperation(method, path, prefix, operation, spec);
      const list = groups.get(built.group) ?? [];
      if (list.some((existing) => existing.name === built.name)) {
        throw new Error(`two operations are named ${built.group}.${built.name}`);
      }
      groups.set(built.group, [...list, built]);
    }
  }

  const router = [...groups]
    .map(([group, operations]) => {
      const members = operations.map(({ name, code }) => `    ${name}: ${code},`).join("\n");
      return `  ${group}: {\n${members}\n  },`;
    })
    .join("\n");

  const source = `/* eslint-disable */
// GENERATED FILE: do not edit.
// Source: openapi.json (\`${spec.info?.title} ${spec.info?.version}\`), produced by \`blog-api --print-openapi\`.
// Regenerate with \`npm run gen\`; \`npm run check:contract\` fails when this file is stale.

import "@orpc/openapi/extensions/route";
import { eventIterator, oc } from "@orpc/contract";
import { z } from "zod";

/** Where the server mounts the API; pass it as the link's \`url\`. */
export const apiPrefix = ${JSON.stringify(prefix || "/")} as \`/\${string}\`;

// ---------------------------------------------------------------- schemas

${components}
// --------------------------------------------------------------- contract

export const contract = {
${router}
};
`;
  // Formatting makes the output readable and, more to the point, stable.
  return format(source, { parser: "typescript", printWidth: 110 });
}

// --------------------------------------------------------------------- main

function openapiFromServer(): string {
  const raw = execFileSync("cargo", ["run", "--quiet", "-p", "blog-api", "--", "--print-openapi"], {
    cwd: workspace,
    stdio: ["ignore", "pipe", "inherit"],
    maxBuffer: 16 * 1024 * 1024,
  }).toString();
  return `${JSON.stringify(JSON.parse(raw), null, 2)}\n`;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const check = process.argv.includes("--check");
  const fromServer = process.argv.includes("--from-server") || check;
  const spec = fromServer ? openapiFromServer() : readFileSync(specPath, "utf8");
  const contract = await generate(JSON.parse(spec));

  if (check) {
    const stale = [
      readFileSync(specPath, "utf8") !== spec && "openapi.json",
      readFileSync(contractPath, "utf8") !== contract && "src/contract.generated.ts",
    ].filter(Boolean);
    if (stale.length > 0) {
      console.error(`✗ out of date: ${stale.join(", ")}. Run \`npm run gen\`.`);
      process.exit(1);
    }
    console.log("✓ openapi.json and the generated contract match the server");
  } else {
    if (fromServer) writeFileSync(specPath, spec);
    writeFileSync(contractPath, contract);
    console.log(`✓ wrote ${fromServer ? "openapi.json and " : ""}src/contract.generated.ts`);
  }
}
