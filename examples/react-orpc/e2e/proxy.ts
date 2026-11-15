import { type Server, type Socket, connect, createServer } from "node:net";

export interface FlakyProxy {
  origin: string;
  /** Refuse (immediately close) new connections until `allow()`. */
  block(): void;
  allow(): void;
  /** Cut every open connection, as a dropped network would. */
  sever(): void;
  close(): Promise<void>;
}

/** A TCP proxy in front of `target` that tests can cut and restore. */
export async function startProxy(target: string): Promise<FlakyProxy> {
  const { hostname, port } = new URL(target);
  const sockets = new Set<Socket>();
  let blocked = false;

  const server: Server = createServer((client) => {
    if (blocked) return void client.destroy();
    const upstream = connect({ host: hostname, port: Number(port) });
    for (const socket of [client, upstream]) {
      sockets.add(socket);
      socket.on("close", () => sockets.delete(socket));
      socket.on("error", () => socket.destroy());
    }
    client.on("close", () => upstream.destroy());
    upstream.on("close", () => client.destroy());
    client.pipe(upstream);
    upstream.pipe(client);
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port: proxyPort } = server.address() as { port: number };

  return {
    origin: `http://127.0.0.1:${proxyPort}`,
    block: () => void (blocked = true),
    allow: () => void (blocked = false),
    sever: () => sockets.forEach((socket) => socket.destroy()),
    close: () => new Promise((resolve) => server.close(() => resolve())),
  };
}
