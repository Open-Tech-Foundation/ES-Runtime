/** The one page this app has. Replace it with yours. */
export function Home() {
  return (
    <>
      <h1>{"{{name}}"}</h1>
      <p className="lede">
        Built with ES Runtime — a secure, standards-based JavaScript runtime from the Open Tech
        Foundation.
      </p>
      <p className="edit">
        Edit <code>src/app/Home.tsx</code> and save.
      </p>
      <nav className="links">
        <a href="https://esrun.opentechf.org/docs">Docs</a>
        <a href="https://esrun.opentechf.org/api">API</a>
        <a href="https://github.com/Open-Tech-Foundation/ES-Runtime">GitHub</a>
      </nav>
    </>
  );
}
