// Route loader — server-only. Its return value is available as router.data on the page.
// https://web.opentechf.org/docs/data-fetching
export default function () {
  return {
    tagline: "Fullstack starter — middleware, loaders, and API routes.",
  };
}