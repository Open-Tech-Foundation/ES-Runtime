export default function Home() {
  let count = $state(0);

  return (
    <section class="welcome">
      <p class="eyebrow">OTF Web</p>
      <h1 class="title">SPA starter</h1>
      <p class="lead">
        Edit <code class="code">app/page.jsx</code> to start building. This template is
        client-first — no API routes, middleware, or server scripts.
      </p>
      <p class="lead">
        Run <code class="code">otfw dev</code> for local development,{" "}
        <code class="code">otfw build</code> for a client bundle, or{" "}
        <code class="code">otfw build --ssg</code> to pre-render static HTML.
      </p>
      <button class="btn" onclick={() => count++}>
        Clicked {count} {count === 1 ? "time" : "times"}
      </button>
    </section>
  );
}