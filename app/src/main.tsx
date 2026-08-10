import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import { registerGlobalCrashLogging } from './utils/crashLogging'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import './i18n'
import PlatformApp from './PlatformApp'

const queryClient = new QueryClient()
const IS_ANDROID_SHELL = import.meta.env.VITE_COWORK_ANDROID === 'true'
const IS_WEB_CLIENT = import.meta.env.MODE === 'web' || import.meta.env.VITE_COWORK_WEB === 'true'

registerGlobalCrashLogging()
if (!IS_ANDROID_SHELL && !IS_WEB_CLIENT) {
  void import('./utils/windowState').then(({ setupWindowStatePersistence }) => setupWindowStatePersistence())
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <PlatformApp />
    </QueryClientProvider>
  </StrictMode>,
)
