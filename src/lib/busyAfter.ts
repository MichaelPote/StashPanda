/**
 * Tells when work has been going on for longer than `delay` milliseconds, so that a spinner
 * shows for an action the user would notice waiting for and not as a flicker on one that is
 * over at once (staging a file takes a few milliseconds on a local disk).
 *
 * The returned function is called when a piece of work starts, and returns the function to
 * call when that piece ends. `set(true)` is called once work has been running for `delay`
 * without a break, and `set(false)` when the last piece ends after that. Pieces may overlap.
 */
export function busyAfter(delay: number, set: (busy: boolean) => void): () => () => void {
  let running = 0;
  let shown = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  return () => {
    running += 1;
    if (running === 1) {
      timer = setTimeout(() => {
        timer = null;
        shown = true;
        set(true);
      }, delay);
    }
    let ended = false;
    return () => {
      // Ending the same piece twice must not end another one.
      if (ended) return;
      ended = true;
      running -= 1;
      if (running > 0) return;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      if (shown) {
        shown = false;
        set(false);
      }
    };
  };
}
