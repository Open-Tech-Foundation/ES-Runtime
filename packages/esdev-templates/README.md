# @opentf/esdev-templates

The project templates [`esdev create`](https://esrun.opentechf.org/esdev/create)
writes. You do not install this package: esdev fetches it, checks it against
the registry's published sha512, and reads the newest release that supports
the esdev you run.

```sh
esdev create my-app                         # the newest templates this esdev reads
esdev create my-app --templates=0.1.0       # a pinned release
esdev create my-app --from=./esdev-templates  # a local copy, for writing templates
```

## Layout

```text
templates/
  catalog.json                 groups some templates are chosen behind
  <name>/
    template.json              label, description, group, options
    …                          the files every project from it gets
    _option/<option>/<value>/  files a choice adds or replaces
```

`template.json` names each option, its question and choices, and either a
`default` or, for a choice that adds a dependency, an `unattended` answer with
no default. A choice may also `omit` paths and add `devDependencies` and
`scripts` to the project's `package.json`. `engines.esdev` in this package's
`package.json` is the range of esdev versions that read its format.

An option asked only for some earlier answers says so with `when`, and what it
is otherwise with `otherwise`:

```json
{ "name": "server", "kind": "confirm", "question": "Add API routes?",
  "default": "no", "when": { "render": ["static", "client"] }, "otherwise": "yes",
  "choices": [ … ] }
```

Overlays nest. A file that depends on two choices sits under both:

```text
_option/language/ts/_option/server/yes/app/todos/page.tsx
```

It is written only when both are chosen, and the most specific file wins a
path: base files, then overlays under one option, then two.
