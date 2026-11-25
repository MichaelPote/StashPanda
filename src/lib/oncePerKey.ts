/**
 * Runs an asynchronous task at most once at a time for each key. A call made while a task for
 * the same key is still running starts nothing and gets that task's promise, so both callers see
 * the same result or the same failure. Once the task has settled, the next call starts a new one.
 *
 * Opening a repository uses this: a double-click in the Repositories list asks for the same
 * repository twice, and two opens at once would each make a session and a backend handle.
 */
export function oncePerKey<T>(): (key: string, task: () => Promise<T>) => Promise<T> {
  const running = new Map<string, Promise<T>>();
  return (key, task) => {
    const existing = running.get(key);
    if (existing) return existing;
    const started = task().finally(() => {
      if (running.get(key) === started) running.delete(key);
    });
    running.set(key, started);
    return started;
  };
}
