export default function Counter({ initial = 0 }) {
  let count = $state(initial);

  return (
    <button class="otfw-counter" type="button" onclick={() => count++} data-testid="counter">
      Count {count}
    </button>
  );
}