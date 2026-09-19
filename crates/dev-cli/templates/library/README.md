# {{name}}

An OTF Web component library, scaffolded by `esdev create`. Reusable
components to publish — not an app.

## Run it

```sh
pnpm install
pnpm run test    # the consumer contract, under esdev test
```

`Counter` in `src/` is a starting point for your own components. Consumers
compile `.jsx` from this package through their app's `otfw` toolchain.

`tests/counter.test.js` asserts the package surface (entry file, re-export,
props) rather than clicks: rendering needs a DOM and the OTF compiler, which
`esdev test` does not provide. A click-through test belongs in a browser
runner, not in this starter.
