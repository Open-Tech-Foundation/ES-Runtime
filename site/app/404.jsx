export default function NotFound() {
  return (
    <div className="flex min-h-[60vh] flex-col items-center justify-center px-6 text-center">
      <p className="font-mono text-sm font-semibold text-brand-600">404</p>
      <h1 className="mt-3 text-3xl font-bold tracking-tight text-zinc-900">
        Page not found
      </h1>
      <p className="mt-3 max-w-md text-zinc-600">
        The page you are looking for does not exist or has moved.
      </p>
      <a
        href="/"
        className="mt-6 rounded-lg bg-brand-600 px-5 py-3 text-sm font-semibold text-white transition-colors hover:bg-brand-500"
      >
        Go home
      </a>
    </div>
  );
}
