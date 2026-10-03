export default function Home() {
  let count = $state(0);

  return (
    <section>
      <h1>Hello, world!</h1>
      <p>
        Edit <code>app/page.jsx</code> and save.
      </p>
      <button type="button" onclick={() => count++}>
        Count {count}
      </button>
    </section>
  );
}
