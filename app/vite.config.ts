import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import path from "path"

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
    warmup: {
      clientFiles: [
        './src/App.tsx',
        './src/components/Layout.tsx',
        './src/components/CoworkView.tsx',
        './src/components/LeftSidebar.tsx',
      ],
    },
  },
  optimizeDeps: {
    include: [
      '@tanstack/react-query',
      '@tauri-apps/api',
      '@tauri-apps/api/core',
      '@tauri-apps/api/event',
      '@tauri-apps/api/window',
      '@tauri-apps/plugin-dialog',
      '@tauri-apps/plugin-notification',
      '@tauri-apps/plugin-opener',
      'class-variance-authority',
      'clsx',
      'i18next',
      'lucide-react',
      'react',
      'react-dom',
      'react-dom/client',
      'react-i18next',
      'react-router-dom',
      'tailwind-merge',
      'zod',
      'zustand',
      'zustand/middleware',
    ],
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  build: {
    rollupOptions: {
      checks: {
        pluginTimings: false,
      },
      output: {
        codeSplitting: {
          groups: [
            {
              name: 'router',
              test: (id) => id.replaceAll('\\', '/').includes('react-router'),
              priority: 120,
              includeDependenciesRecursively: false,
            },
            {
              name: 'i18n',
              test: (id) => id.replaceAll('\\', '/').endsWith('/src/i18n.ts'),
              priority: 120,
              includeDependenciesRecursively: false,
            },
            {
              name: 'query',
              test: (id) => id.replaceAll('\\', '/').includes('@tanstack/react-query'),
              priority: 110,
              includeDependenciesRecursively: false,
            },
            {
              name: 'tauri',
              test: (id) => id.replaceAll('\\', '/').includes('@tauri-apps'),
              priority: 110,
              includeDependenciesRecursively: false,
            },
            {
              name: 'terminal-vendor',
              test: (id) => id.replaceAll('\\', '/').includes('@xterm'),
              priority: 110,
              includeDependenciesRecursively: false,
            },
            {
              name: 'icons',
              test: (id) => id.replaceAll('\\', '/').includes('lucide-react'),
              priority: 110,
              includeDependenciesRecursively: false,
            },
            {
              name: 'react-vendor',
              test: (id) => {
                const normalizedId = id.replaceAll('\\', '/')
                return normalizedId.includes('/node_modules/react/')
                  || normalizedId.includes('/node_modules/react-dom/')
                  || normalizedId.includes('/node_modules/scheduler/')
              },
              priority: 110,
              includeDependenciesRecursively: false,
            },
            {
              name: 'engine-core',
              test: /[\\/]src[\\/]engine[\\/]/,
              priority: 100,
              includeDependenciesRecursively: false,
            },
            {
              name: 'cowork-support',
              test: (id) => {
                const normalizedId = id.replaceAll('\\', '/')
                return normalizedId.includes('/src/stores/commandRegistryStore')
                  || normalizedId.includes('/src/stores/coworkStore')
                  || normalizedId.includes('/src/utils/attachmentPromptContext')
                  || normalizedId.includes('/src/utils/chatAttachments')
              },
              priority: 100,
              includeDependenciesRecursively: false,
            },
            {
              name: 'vendor',
              test: /[\\/]node_modules[\\/]/,
              priority: 10,
              includeDependenciesRecursively: false,
            },
          ],
        },
      },
    },
  },
})
