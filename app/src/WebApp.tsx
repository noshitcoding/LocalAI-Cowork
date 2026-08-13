import { lazy, Suspense } from 'react'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom'

import './App.css'

const OidcCallbackPage = lazy(() => import('./components/OidcCallbackPage'))
const RemoteServerView = lazy(() => import('./components/RemoteServerView'))

export default function WebApp() {
  return (
    <BrowserRouter>
      <Suspense fallback={<main className="remote-server-view" aria-busy="true" />}>
        <Routes>
          <Route path="/auth/callback" element={<OidcCallbackPage />} />
          <Route path="/server" element={<RemoteServerView />} />
          <Route path="*" element={<Navigate to="/server" replace />} />
        </Routes>
      </Suspense>
    </BrowserRouter>
  )
}
