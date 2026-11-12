// GET /api/hello
export function GET(_request: Request) {
  return Response.json({ message: "Hello, world!" });
}
