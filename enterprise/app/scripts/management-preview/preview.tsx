import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import EnterpriseManagement from '../../src/components/EnterpriseManagement'
import i18n from '../../src/i18n'
import '../../src/index.css'
import '../../src/styles/design-tokens.css'
import '../../src/App.css'
import '../../src/styles/flat-contract.css'
import './preview.css'

const params = new URLSearchParams(window.location.search)
document.documentElement.dataset.theme = params.get('theme') === 'dark' ? 'dark' : 'light'
await i18n.changeLanguage(params.get('lang') === 'en' ? 'en' : 'de')
// No API or provider request may leave this fixture. Source modules render real
// components against the local client alias, and unknown requests fail closed.
globalThis.fetch = async () => { throw new Error('TESTDATA preview: network requests are disabled') }
createRoot(document.getElementById('root')!).render(<BrowserRouter><div className="management-preview-banner"><strong>TESTDATEN</strong><span>Lokale Designvorschau · synthetische Organisationen und Instanzen · keine Verbindung zur Live-Verwaltung</span></div><EnterpriseManagement /></BrowserRouter>)
