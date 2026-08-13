import { useEffect } from 'react'
import { useNavigate } from 'react-router-dom'

import { useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'

export default function OidcCallbackPage() {
  const restore = useRemoteRuntimeStore((state) => state.restore)
  const error = useRemoteRuntimeStore((state) => state.error)
  const navigate = useNavigate()

  useEffect(() => {
    void restore().then((authenticated) => {
      if (authenticated) navigate('/server', { replace: true })
    })
  }, [navigate, restore])

  return <main className="remote-server-view remote-server-login"><div className="remote-login-card"><h1>Completing single sign-on…</h1>{error ? <div className="remote-inline-error" role="alert">{error}</div> : <p>Validating the identity-provider response.</p>}</div></main>
}
