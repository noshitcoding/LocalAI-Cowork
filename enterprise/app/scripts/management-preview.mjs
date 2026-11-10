import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import { createServer } from 'vite'
import react from '@vitejs/plugin-react'

const scriptDirectory = dirname(fileURLToPath(import.meta.url))
const previewRoot = resolve(scriptDirectory, 'management-preview')
const appRoot = resolve(scriptDirectory, '..')
const fixture = name => resolve(previewRoot, name)
// These aliases exist only in this separate development server. Production
// authentication and API modules are never changed or bundled by this harness.
const server = await createServer({
  configFile: false,
  root: previewRoot,
  mode: 'management-preview',
  cacheDir: resolve(appRoot, 'node_modules/.vite-management-preview'),
  plugins: [{
    name: 'isolated-management-test-fixtures', enforce: 'pre',
    resolveId(source) {
      if (/\/managementClient(?:\.ts)?$/.test(source)) return fixture('client.ts')
      if (/\/remoteRuntimeStore(?:\.ts)?$/.test(source)) return fixture('store.ts')
      if (/\/RemoteServerView(?:\.tsx)?$/.test(source)) return fixture('RemoteServerView.tsx')
    },
  }, react()],
  resolve: { alias: { '@': resolve(appRoot, 'src') } },
  server: { host: '127.0.0.1', port: 18085, strictPort: true, fs: { allow: [appRoot] } },
})
await server.listen()
console.log('TESTDATA: isolated management preview at http://127.0.0.1:18085/ — no live API access')
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, async () => { await server.close(); process.exit(0) })
