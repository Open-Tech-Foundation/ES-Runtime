import { router } from "@opentf/web";

export default function Home() {
  let count = $state(0);
  let apiMessage = $state("");

  async function pingApi() {
    try {
      const res = await fetch("/api/hello");
      apiMessage = JSON.stringify(await res.json(), null, 2);
    } catch {
      apiMessage = "API unavailable — run `pnpm run dev` or `pnpm run serve`.";
    }
  }

  return (
    <section class="welcome">
      <p class="eyebrow">OTF Web</p>
      <h1 class="title">Fullstack starter</h1>
      <p class="lead">{router.data?.tagline}</p>
      <p class="lead">
        <code class="code">app/_middleware.js</code> stamps{" "}
        <code class="code">context.locals</code>.{" "}
        <code class="code">app/loader.js</code> feeds this page via{" "}
        <code class="code">router.data</code>.{" "}
        <code class="code">app/api/hello/route.js</code> is a sample API route.
      </p>
      <button class="btn" onclick={() => count++}>
        Clicked {count} {count === 1 ? "time" : "times"}
      </button>
      <button class="btn" onclick={pingApi}>
        Ping /api/hello
      </button>
      {apiMessage && <pre class="api-out">{apiMessage}</pre>}
    </section>
  );
}
