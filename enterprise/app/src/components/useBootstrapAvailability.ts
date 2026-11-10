import { useEffect, useMemo, useState } from 'react'
import { getBootstrapAvailability } from '../runtime/authBootstrap'

export function useBootstrapAvailability(serverUrl: string, enabled: boolean): boolean | null {
  const request = useMemo(() => ({ serverUrl, enabled }), [serverUrl, enabled])
  const [result, setResult] = useState<{ request: object; available: boolean } | null>(null)
  useEffect(() => {
    if (!request.enabled || !request.serverUrl) return
    const abort = new AbortController()
    let pending = false
    let complete = false
    const refresh = async () => {
      if (pending || complete || abort.signal.aborted) return
      pending = true
      try {
        const available = await getBootstrapAvailability(request.serverUrl, abort.signal)
        if (!abort.signal.aborted) {
          setResult({ request, available })
          if (!available) { complete = true; window.clearInterval(timer) }
        }
      } catch {
        // Login still works on older/unreachable servers. Only a confirmed true
        // response exposes first-admin setup; errors never infer server state.
      } finally { pending = false }
    }
    const timer = window.setInterval(() => void refresh(), 15_000)
    void refresh()
    const onFocus = () => void refresh()
    window.addEventListener('focus', onFocus)
    return () => { abort.abort(); window.clearInterval(timer); window.removeEventListener('focus', onFocus) }
  }, [request])
  return enabled && result !== null && result.request === request ? result.available : null
}
