import { lazy, Suspense, useEffect } from 'react'

const IS_ANDROID_SHELL = import.meta.env.VITE_COWORK_ANDROID === 'true'
const IS_WEB_CLIENT = import.meta.env.MODE === 'web' || import.meta.env.VITE_COWORK_WEB === 'true'
const PlatformRoot = lazy(() => {
  if (IS_ANDROID_SHELL) return import('./mobile/MobileApp')
  if (IS_WEB_CLIENT) return import('./WebApp')
  return import('./App')
})

export default function PlatformApp() {
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
