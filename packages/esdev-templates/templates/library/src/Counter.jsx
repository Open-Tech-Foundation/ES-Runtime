export default function Counter({ initial = 0 }) {
  let count = $state(initial);

  return (
    <button class="afw-counter" type="button" onclick={() => count++} data-testid="counter">
      Count {count}
    </button>
  );
}