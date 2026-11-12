import { router } from "@opentf/afw";

export default function Home() {
  return (
    <section>
      <h1>{router.data?.message}</h1>
      <p>
        Edit <code>app/page.tsx</code>. The message comes from <code>app/loader.ts</code>.
        Separately, <code>app/api/hello/route.ts</code> serves <code>/api/hello</code> as
        its own example — nothing on this page fetches it.
      </p>
    </section>
  );
}
