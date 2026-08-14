/**
 * The one resource this API serves, and the store behind it.
 *
 * The store is a `Map`, and it is meant to be replaced — by `runtime:db`, by a
 * fetch to a service, by whatever this actually talks to. What is worth keeping
 * is the shape: **validation before storage, and a handler that does not know
 * which of those it is talking to.**
 *
 * # The validation is hand-written on purpose
 *
 * A schema library is a good choice for a real API and a bad one for a template:
 * it would be the first dependency, and it would hide the thing worth showing —
 * that a request from outside is *untrusted input*, checked field by field,
 * before anything else sees it. Twenty lines here is the honest amount.
 */
import { HttpError, json, noContent, readJson } from "./http.ts";
import type { Context } from "./router.ts";

export type Task = {
  id: string;
  title: string;
  done: boolean;
  created: string;
};

/** Stands in for a database. Replacing it should not change a handler. */
const tasks = new Map<string, Task>();

// Two rows, so a fresh `GET /tasks` shows something.
for (const title of ["Read the README", "Narrow the permissions"]) {
  const id = crypto.randomUUID();
  tasks.set(id, { id, title, done: false, created: new Date().toISOString() });
}

export function listTasks(): Response {
  const all = [...tasks.values()].sort((a, b) => a.created.localeCompare(b.created));
  return json({ tasks: all });
}

export function showTask({ params }: Context): Response {
  const task = tasks.get(params.id!);
  if (!task) {
    throw HttpError.notFound(`No task ${params.id}`);
  }
  return json({ task });
}

export async function createTask({ request }: Context): Promise<Response> {
  const body = await readJson(request);
  const title = validateTitle(body);

  const task: Task = {
    id: crypto.randomUUID(),
    title,
    done: false,
    created: new Date().toISOString(),
  };
  tasks.set(task.id, task);

  // 201 and a `Location`, which is what tells a client where the thing it just
  // made now lives.
  return json({ task }, { status: 201, headers: { location: `/tasks/${task.id}` } });
}

export function deleteTask({ params }: Context): Response {
  if (!tasks.delete(params.id!)) {
    throw HttpError.notFound(`No task ${params.id}`);
  }
  return noContent();
}

/**
 * The one field a task is created with, checked.
 *
 * Exported because this is the part worth testing: everything above it is a
 * `Map`, and this is where a request from outside stops being untrusted.
 */
export function validateTitle(body: unknown): string {
  if (typeof body !== "object" || body === null || Array.isArray(body)) {
    throw HttpError.badRequest("Expected a JSON object");
  }
  const title = (body as { title?: unknown }).title;

  if (typeof title !== "string") {
    throw HttpError.invalid({ title: "must be a string" });
  }
  const trimmed = title.trim();
  if (trimmed.length === 0) {
    throw HttpError.invalid({ title: "must not be empty" });
  }
  if (trimmed.length > 200) {
    // Bounded because it is stored. An unbounded field from an unauthenticated
    // client is how a store fills up.
    throw HttpError.invalid({ title: "must be 200 characters or fewer" });
  }
  return trimmed;
}
