/**
 * A minimal reactive counter — a starting point for your own components.
 * Consumers compile `.jsx` from this package through their app's `otfw` toolchain.
 */
export default function Counter({ initial = 0 }) {
  let count = $state(initial);

  return (
    <button class="otfw-counter" type="button" onclick={() => count++} data-testid="counter">
      Count {count}
    </button>
  );
}