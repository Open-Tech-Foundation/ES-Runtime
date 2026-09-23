import { router } from "@opentf/web";

export default function Home() {
  return (
    <section>
      <h1>{router.data?.message}</h1>
      <p>
        Edit <code>app/page.jsx</code>. The message comes from <code>app/loader.js</code>, and{" "}
        <code>app/api/hello/route.js</code> serves <code>/api/hello</code>.
      </p>
    </section>
  );
}
