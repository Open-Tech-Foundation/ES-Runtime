export default function RootLayout({ children }: { children: unknown }) {
  return (
    <>
      <nav>
        <a href="/">Home</a> · <a href="/todos">Todos</a>
      </nav>
      <main>{children}</main>
    </>
  );
}
