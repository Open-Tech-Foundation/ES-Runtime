export default function RootLayout({ children }) {
  return (
    <>
      <nav>
        <a href="/">Home</a> · <a href="/todos">Todos</a>
      </nav>
      <main>{children}</main>
    </>
  );
}
