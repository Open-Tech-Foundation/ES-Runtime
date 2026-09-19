export default function Home() {
  return (
    <main class="landing">
      <section class="hero">
        <p class="eyebrow">OTF Web Docs</p>
        <h1>Build fast docs with MDX and native Web Components.</h1>
        <p class="lead">
          Start with a focused landing page, then write your documentation in MDX
          with generated navigation, sidebar, table of contents, and static output.
        </p>
        <div class="actions">
          <a href="/docs" class="btn primary">Read the docs</a>
          <a class="btn secondary" href="https://github.com/Open-Tech-Foundation/Web-App-Framework" target="_blank" rel="noreferrer">
            GitHub
          </a>
        </div>
      </section>

      <section class="features" aria-label="Highlights">
        <article>
          <h2>MDX Content</h2>
          <p>Author pages with Markdown, JSX components, frontmatter, and highlighted code blocks.</p>
        </article>
        <article>
          <h2>Generated Navigation</h2>
          <p>Use <code>_meta.js</code> to control sidebar order while the docs plugin builds the tree.</p>
        </article>
        <article>
          <h2>Static Output</h2>
          <p>Ship a pre-rendered site with the same client router and native runtime.</p>
        </article>
      </section>
    </main>
  );
}
