import { defineConfig } from 'vite'
import { babel } from '@rollup/plugin-babel'
import tailwindcss from '@tailwindcss/vite'
import { readFileSync } from 'node:fs'

// The displayed runtime version comes from the workspace Cargo.toml (the single
// source of truth, bumped by scripts/release.sh). This site's own package.json
// version is unrelated — docs change far more often than the runtime.
const cargoToml = readFileSync(new URL('../Cargo.toml', import.meta.url), 'utf8')
const runtimeVersion = cargoToml.match(/^version = "([^"]+)"/m)?.[1] ?? '0.0.0'

export default defineConfig({
  define: {
    __RUNTIME_VERSION__: JSON.stringify(runtimeVersion),
  },
  esbuild: {
    jsx: 'preserve'
  },
  optimizeDeps: {
    rolldownOptions: {
      transform: {
        jsx: 'preserve'
      }
    }
  },
  plugins: [
    tailwindcss(),
    {
      ...babel({
        babelHelpers: 'bundled',
        extensions: ['.js', '.jsx', '.ts', '.tsx'],
        exclude: 'node_modules/**',
        configFile: false,
        plugins: [
          "@babel/plugin-syntax-jsx",
          ["@opentf/web/compiler"]
        ]
      }),
      enforce: 'pre'
    }
  ]
})
