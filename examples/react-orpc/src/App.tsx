import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, type FormEvent } from "react";

import { orpc } from "./client";
import type { PostEventDto } from "./contract.generated";
import { describeError } from "./errors-ui";

export function App() {
  return (
    <main>
      <h1>nestrs · oRPC · TanStack Query</h1>
      <p className="lede">
        A typed client built from a contract, talking to a Rust (Axum) server over plain REST and
        Server-Sent Events.
      </p>
      <div className="grid">
        <Authors />
        <Posts />
        <LiveFeed />
      </div>
    </main>
  );
}

function ErrorLine({ error }: { error: unknown }) {
  return error ? <p className="error">{describeError(error)}</p> : null;
}

function Authors() {
  const queryClient = useQueryClient();
  const users = useQuery(orpc.users.list.queryOptions({ input: { limit: 50 } }));
  const create = useMutation(
    orpc.users.create.mutationOptions({
      onSuccess: () => queryClient.invalidateQueries({ queryKey: orpc.users.list.key() }),
    }),
  );

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    create.mutate({ name: String(form.get("name")), email: String(form.get("email")) });
  }

  return (
    <section>
      <h2>Authors</h2>
      <form onSubmit={submit}>
        <input name="name" placeholder="Name" required />
        <input name="email" placeholder="Email" required />
        <button disabled={create.isPending}>Register</button>
      </form>
      <ErrorLine error={create.error} />
      <ErrorLine error={users.error} />
      <ul>
        {users.data?.map((user) => (
          <li key={user.id}>
            <strong>{user.name}</strong> <small>{user.email}</small>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Posts() {
  const queryClient = useQueryClient();
  const users = useQuery(orpc.users.list.queryOptions({ input: { limit: 50 } }));
  const posts = useQuery(orpc.posts.list.queryOptions({ input: { limit: 50 } }));
  const refresh = () => queryClient.invalidateQueries({ queryKey: orpc.posts.list.key() });
  const create = useMutation(orpc.posts.create.mutationOptions({ onSuccess: refresh }));
  const remove = useMutation(orpc.posts.remove.mutationOptions({ onSuccess: refresh }));

  // Everyone who publishes or deletes, in any tab, refreshes this list.
  const received = useFeed().data?.length ?? 0;
  useEffect(() => {
    if (received > 0) void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [received]);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    create.mutate({
      author_id: String(form.get("author")),
      title: String(form.get("title")),
      body: String(form.get("body")),
    });
  }

  return (
    <section>
      <h2>Posts</h2>
      <form onSubmit={submit}>
        <select name="author" required defaultValue="">
          <option value="" disabled>
            Author…
          </option>
          {users.data?.map((user) => (
            <option key={user.id} value={user.id}>
              {user.name}
            </option>
          ))}
        </select>
        <input name="title" placeholder="Title" required />
        <textarea name="body" placeholder="Body" required />
        <button disabled={create.isPending}>Publish</button>
      </form>
      <ErrorLine error={create.error} />
      <ErrorLine error={remove.error} />
      <ErrorLine error={posts.error} />
      <ul>
        {posts.data?.map((post) => (
          <li key={post.id}>
            <strong>{post.title}</strong>
            <p>{post.body}</p>
            <button className="link" onClick={() => remove.mutate({ id: post.id })}>
              delete
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * The typed event stream as TanStack data. `streamedOptions` opens the SSE request,
 * appends every message to `data` and marks the query `success` when the server
 * sends `close`. Each item is a `PostEventDto`, so the union narrows in the UI.
 */
function useFeed() {
  // Same query key everywhere, so all components share one connection.
  return useQuery(orpc.posts.events.streamedOptions({ queryFnOptions: { maxChunks: 20 } }));
}

function LiveFeed() {
  const feed = useFeed();
  const events = feed.data ?? [];
  const state = feed.error ? "error" : feed.fetchStatus === "fetching" ? "live" : "closed";

  return (
    <section>
      <h2>
        Live feed <span className={`badge ${state}`}>{state}</span>
      </h2>
      <ErrorLine error={feed.error} />
      {state === "closed" && (
        <button onClick={() => feed.refetch()}>Reconnect</button>
      )}
      <ol reversed>
        {[...events].reverse().map((event, index) => (
          <li key={index}>{render(event)}</li>
        ))}
      </ol>
      {events.length === 0 && <p className="muted">Waiting for events…</p>}
    </section>
  );
}

/** Exhaustive over the event union: add a variant to the contract and this stops compiling. */
function render(event: PostEventDto): string {
  switch (event.type) {
    case "published":
      return `published “${event.post.title}”`;
    case "removed":
      return `removed ${event.id.slice(0, 8)}`;
  }
}
