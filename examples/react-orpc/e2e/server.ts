import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";

const workspace = fileURLToPath(new URL("../../..", import.meta.url));

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address() as { port: number };
      probe.close(() => resolve(port));
    });
  });
}

export interface RunningServer {
  origin: string;
  process: ChildProcess;
  /** Resolves with the exit code once the process has stopped. */
  exited: Promise<number | null>;
  stop(): Promise<void>;
}

/** Builds and starts the real Rust `blog-api` binary on a free port, in memory mode. */
export async function startServer(): Promise<RunningServer> {
  execFileSync("cargo", ["build", "--quiet", "-p", "blog-api"], { cwd: workspace, stdio: "inherit" });
  const port = await freePort();
  const child = spawn(`${workspace}/target/debug/blog-api`, [], {
    cwd: workspace,
    env: { ...process.env, BLOG_BIND_ADDR: `127.0.0.1:${port}`, RUST_LOG: "warn" },
    stdio: ["ignore", "inherit", "inherit"],
  });
  const exited = new Promise<number | null>((resolve) => child.once("exit", (code) => resolve(code)));
  const origin = `http://127.0.0.1:${port}`;

  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      if ((await fetch(`${origin}/api/health/live`)).ok) break;
    } catch {
      /* not listening yet */
    }
    if (child.exitCode !== null) throw new Error(`blog-api exited early with ${child.exitCode}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }

  return {
    origin,
    process: child,
    exited,
    async stop() {
      if (child.exitCode === null) child.kill("SIGTERM");
      await exited;
    },
  };
}
