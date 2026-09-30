# @opentf/esdev-plugin-react

React Fast Refresh for esdev's current plugin contract (`ctx.hot` and
`ctx.platform`). Install alongside React in your development dependencies:

```sh
npm install --save-dev @opentf/esdev-plugin-react react-refresh
```

Declare it once in `esdev.json`:

```json
{
  "plugins": ["@opentf/esdev-plugin-react"],
  "jsx": { "importSource": "react", "reactCompiler": true },
  "build": {
    "targets": { "web": { "entry": "index.html", "outdir": "dist" } }
  }
}
```

Run `esdev start`. The plugin instruments `.jsx` and `.tsx` modules in hot
browser builds and initializes React's refresh runtime automatically. A
separate refresh bootstrap in your application entry is unnecessary. Release
builds, server targets, tests, and `--no-hot` builds receive no refresh wrapper.

For a programmatic build, import the default plugin object and place it in
`plugins`. It activates only when the hook context describes a hot browser
build. JSX compilation remains configured through esdev's `jsx` settings.

This package has its own version and requires `react-refresh` 0.19.x. Its
initial release has not been published; the embedded React templates continue
to work independently until their package dependency is migrated.
