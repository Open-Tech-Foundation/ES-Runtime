import { defineConfig } from 'vite'
import { babel } from '@rollup/plugin-babel'
import tailwindcss from '@tailwindcss/vite'

// The displayed runtime version lives in src/runtime-version.js (a committed
// module synced from the workspace Cargo.toml by scripts/release.sh) — imported
// like any source, so this config reads no files at build time and stays
// portable to any static host.
export default defineConfig({
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
