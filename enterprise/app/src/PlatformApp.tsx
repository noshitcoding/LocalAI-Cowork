import { lazy, Suspense, useEffect } from 'react'

import { useAppliedTheme } from './hooks/useAppliedTheme'

const IS_ANDROID_SHELL = import.meta.env.VITE_COWORK_ANDROID === 'true'
const IS_MOBILE_PREVIEW = import.meta.env.VITE_COWORK_MOBILE_PREVIEW === 'true'
const IS_WEB_CLIENT = import.meta.env.MODE === 'web' || import.meta.env.VITE_COWORK_WEB === 'true'
const PlatformRoot = lazy(() => {
  if (IS_ANDROID_SHELL || IS_MOBILE_PREVIEW) return import('./mobile/MobileApp')
  if (IS_WEB_CLIENT) return import('./WebApp')
  return import('./App')
})

export default function PlatformApp() {
  useAppliedTheme({ audit: !IS_ANDROID_SHELL && !IS_MOBILE_PREVIEW })

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      const loader = document.getElementById('boot-loader')
      if (!loader) return
      loader.classList.add('boot-loader-hidden')
      window.setTimeout(() => loader.remove(), 220)
    })
    return () => window.cancelAnimationFrame(frame)
  }, [])

  return <Suspense fallback={null}><PlatformRoot /></Suspense>
}
