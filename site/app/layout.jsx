import Nav from "../components/Nav.jsx";
import Footer from "../components/Footer.jsx";

export default function RootLayout({ children }) {
  return (
    <div className="flex min-h-screen flex-col bg-white text-zinc-900">
      <Nav />
      <main className="flex-1 pt-16">{children}</main>
      <Footer />
    </div>
  );
}
