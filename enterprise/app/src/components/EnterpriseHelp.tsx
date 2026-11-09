import { Info } from 'lucide-react'
import { useId, useState } from 'react'

export default function EnterpriseHelp({ label, children }: { label: string; children: string }) {
  const [open, setOpen] = useState(false)
  const id = useId()
  return <span className="enterprise-field-help"><button type="button" aria-label={label} aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)} onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); setOpen(false) } }}><Info size={13} aria-hidden="true" /></button>{open && <span id={id} role="note">{children}</span>}</span>
}
